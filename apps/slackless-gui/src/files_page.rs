//! The rail's Files destination: Slack's Files view in the main area, beside the sidebar's
//! Files list column ([`crate::sidebar`]). One page per list-column row (All files,
//! Canvases, Lists): the title, a search field, Slack's All / Created by you / Shared with
//! you chips, and the file rows - type tile, bold name, owner and date - in one bordered
//! card; a row opens its file in the browser, as the Files & links tab does.
//!
//! Live mode fetches a page's rows when the view opens, or when a page whose rows are not
//! fresh is shown: one `files.list` page of at most [`MAX_ROWS`] files, newest first, cached
//! per [`Source`] while the view stays open. The chips and the search filter the loaded rows
//! client-side ([`visible_files`]), so neither ever refetches. The sample workspace derives
//! its rows from the fixture messages' files.
//!
//! Slack's rows read "Last viewed ..."; no mapped API carries view history, so these rows
//! say when the file was created. Slack's "+ New" button, Templates, the welcome banner, the
//! type and sort dropdowns, and each row's avatars, star and menu are left out: none has a
//! working path here.

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use slackless_core::api::FilesFilter;
use slackless_core::types::{File, UserId};

use crate::files::file_icon;
use crate::fixtures::Fixtures;
use crate::messages::file_day;

/// Rows a source keeps: one `files.list` page, newest first.
const MAX_ROWS: usize = 200;
/// The page column's width cap; wider windows center it, as Slack does.
const COLUMN_WIDTH: f32 = 720.;
/// `files.list` `types` value for canvases (observed working, not in Slack's documented
/// list; docs/research/api-canvases-lists.md).
const CANVAS_TYPES: &str = "canvas";

/// The Files list column's rows, top to bottom: one page of the Files view each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilesSection {
    All,
    Canvases,
    Lists,
}

impl FilesSection {
    pub const ALL: [FilesSection; 3] = [Self::All, Self::Canvases, Self::Lists];

    /// The list-column row's label and the page's title.
    pub fn title(self) -> &'static str {
        match self {
            Self::All => "All files",
            Self::Canvases => "Canvases",
            Self::Lists => "Lists",
        }
    }

    /// The list-column row's icon.
    pub fn icon(self) -> IconName {
        match self {
            Self::All => IconName::FileStack,
            Self::Canvases => IconName::StickyNote,
            Self::Lists => IconName::Table2,
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::All => "Search files",
            Self::Canvases => "Search canvases",
            Self::Lists => "Search lists",
        }
    }

    /// Whether `file` belongs on this page: canvases are `quip` files, lists `list` files.
    fn holds(self, file: &File) -> bool {
        match self {
            Self::All => true,
            Self::Canvases => file.filetype.as_deref() == Some("quip"),
            Self::Lists => file.filetype.as_deref() == Some("list"),
        }
    }

    fn source(self) -> Source {
        match self {
            Self::All | Self::Lists => Source::Workspace,
            Self::Canvases => Source::Canvases,
        }
    }

    /// The empty page: Slack's title over a line on what the page lists.
    fn empty_state(self) -> (&'static str, &'static str) {
        match self {
            Self::All => (
                "No files yet",
                "Files you upload or that are shared with you will be shown here.",
            ),
            Self::Canvases => (
                "No canvases yet",
                "Canvases you create or that are shared with you will be shown here.",
            ),
            Self::Lists => (
                "No lists yet",
                "Lists you create or that are shared with you will be shown here.",
            ),
        }
    }
}

/// Slack's ownership chips over a page's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilesChip {
    All,
    /// Files the signed-in user uploaded or created.
    CreatedByYou,
    /// Everyone else's files the signed-in user can see.
    SharedWithYou,
}

impl FilesChip {
    const ALL: [FilesChip; 3] = [Self::All, Self::CreatedByYou, Self::SharedWithYou];

    fn label(self, section: FilesSection) -> &'static str {
        match (self, section) {
            (Self::All, FilesSection::All) => "All",
            (Self::All, FilesSection::Canvases) => "All Canvases",
            (Self::All, FilesSection::Lists) => "All Lists",
            (Self::CreatedByYou, _) => "Created by you",
            (Self::SharedWithYou, _) => "Shared with you",
        }
    }
}

/// Where a page's rows come from: one `files.list` call each. All files and Lists share the
/// workspace-wide list (no `types` value for lists is verified, so Lists filters it);
/// Canvases asks Slack for canvases only, so newer uploads never crowd them out of the cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Workspace,
    Canvases,
}

impl Source {
    const ALL: [Source; 2] = [Source::Workspace, Source::Canvases];

    fn filter(self) -> FilesFilter {
        match self {
            Source::Workspace => FilesFilter::workspace(),
            Source::Canvases => FilesFilter {
                types: Some(CANVAS_TYPES.to_owned()),
                ..FilesFilter::workspace()
            },
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One source's loaded rows and how its last load went.
#[derive(Default)]
struct Rows {
    files: Vec<File>,
    /// Each file's "<owner> · Created <when>" line, resolved when the rows land (never per
    /// frame).
    meta: Vec<SharedString>,
    load: Load,
    /// Asked for since the view last opened; a stale source refetches when its page shows.
    fresh: bool,
}

#[derive(Default)]
enum Load {
    #[default]
    Idle,
    Loading,
    Failed(String),
}

pub enum FilesPageEvent {
    /// Fetch a page's rows (`files.list`); the answer lands through [`FilesPage::land`].
    Load(FilesFilter),
}

pub struct FilesPage {
    fixtures: Arc<Fixtures>,
    section: FilesSection,
    chip: FilesChip,
    search: Entity<TextareaState>,
    /// The search, trimmed and lowercased; empty lists every row.
    query: String,
    /// Per [`Source`], its rows.
    rows: [Rows; 2],
    /// Indices into the shown source's rows that the page lists, recomputed when the rows,
    /// the page, the chip or the search change.
    shown: Vec<usize>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FilesPageEvent> for FilesPage {}

impl FilesPage {
    pub fn new(fixtures: Arc<Fixtures>, window: &mut Window, cx: &mut App) -> Entity<FilesPage> {
        cx.new(|cx| {
            let search = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(FilesSection::All.placeholder())
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![cx.subscribe(
                &search,
                |this: &mut FilesPage, search, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        this.query = search.read(cx).value().trim().to_lowercase();
                        this.refilter();
                        cx.notify();
                    }
                },
            )];
            let mut page = FilesPage {
                fixtures,
                section: FilesSection::All,
                chip: FilesChip::All,
                search,
                query: String::new(),
                rows: Default::default(),
                shown: Vec::new(),
                _subscriptions: subscriptions,
            };
            page.reload();
            page
        })
    }

    /// The rail opened the view: fresh owner names, and in live mode a refetch of the shown
    /// page (the other source refetches when its page is next shown).
    pub fn open(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        self.reload();
        for rows in &mut self.rows {
            rows.fresh = false;
        }
        self.fetch(cx);
        cx.notify();
    }

    /// Snapshot refresh: re-resolve owner names (and re-derive the sample workspace's rows).
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        self.reload();
        cx.notify();
    }

    /// Show `section`'s page with its chips and search reset, fetching it when stale.
    pub fn show_section(
        &mut self,
        section: FilesSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if section == self.section {
            return;
        }
        self.section = section;
        self.chip = FilesChip::All;
        self.query.clear();
        self.search.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.set_placeholder(section.placeholder(), window, cx);
        });
        self.fetch(cx);
        self.refilter();
        cx.notify();
    }

    /// A workspace-wide `files.list` answer: the rows of the source that asked for it.
    pub fn land(
        &mut self,
        filter: &FilesFilter,
        files: &Result<Vec<File>, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = Source::ALL
            .into_iter()
            .find(|source| source.filter() == *filter)
        else {
            return;
        };
        let rows = &mut self.rows[source.index()];
        match files {
            Ok(files) => {
                rows.files = files.iter().take(MAX_ROWS).cloned().collect();
                rows.meta = metas(&self.fixtures, &rows.files);
                rows.load = Load::Idle;
            }
            Err(error) => {
                rows.load = Load::Failed(error.clone());
                rows.fresh = false;
            }
        }
        if source == self.section.source() {
            self.refilter();
        }
        cx.notify();
    }

    /// The error row's retry: fetch the shown page again.
    fn retry(&mut self, cx: &mut Context<Self>) {
        self.rows[self.section.source().index()].fresh = false;
        self.fetch(cx);
        cx.notify();
    }

    /// Live mode: ask for the shown page's rows unless they are fresh (asked for since the
    /// view opened); rows already on screen stay while the refresh is in flight.
    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            return;
        }
        let source = self.section.source();
        let rows = &mut self.rows[source.index()];
        if rows.fresh {
            return;
        }
        rows.fresh = true;
        rows.load = if rows.files.is_empty() {
            Load::Loading
        } else {
            Load::Idle
        };
        cx.emit(FilesPageEvent::Load(source.filter()));
    }

    /// Re-derive what depends on the snapshot: the sample workspace's rows and every row's
    /// owner line.
    fn reload(&mut self) {
        if self.fixtures.sample {
            let files = fixture_files(&self.fixtures);
            for rows in &mut self.rows {
                rows.files = files.clone();
            }
        }
        for rows in &mut self.rows {
            rows.meta = metas(&self.fixtures, &rows.files);
        }
        self.refilter();
    }

    fn refilter(&mut self) {
        let rows = &self.rows[self.section.source().index()];
        self.shown = visible_files(
            &rows.files,
            self.section,
            self.chip,
            &self.fixtures.self_user,
            &self.query,
        );
    }

    /// Slack's search box: a full-width field led by the magnifier.
    fn search_field(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        h_flex()
            .h(px(34.))
            .px_2p5()
            .gap_2()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(
                Icon::new(IconName::Search)
                    .text_color(theme.muted_foreground)
                    .size(px(16.)),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Textarea::new(&self.search)
                        .appearance(false)
                        .bordered(false),
                ),
            )
    }

    /// Slack's ownership chips; the chosen one is filled.
    fn chips(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (fill, border, hover) = (theme.link, theme.border, theme.accent);
        let on_fill = crate::theme::avatar_foreground();
        h_flex().gap_1p5().children(FilesChip::ALL.map(|chip| {
            let active = chip == self.chip;
            div()
                .id(SharedString::from(format!("files-chip-{chip:?}")))
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .when(active, |chip| {
                    chip.border_color(fill).bg(fill).text_color(on_fill)
                })
                .when(!active, |chip| {
                    chip.border_color(border).hover(move |chip| chip.bg(hover))
                })
                .child(chip.label(self.section))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.chip = chip;
                    this.refilter();
                    cx.notify();
                }))
        }))
    }

    /// A failed load: Slack's error text and a retry, above whatever rows are still shown.
    fn error_row(error: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (dim, border) = (theme.muted_foreground, theme.border);
        h_flex()
            .id("files-error")
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(border)
            .text_color(dim)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(format!("Could not load files: {error}")),
            )
            .child(
                Button::new("files-retry")
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::RotateCw)
                    .tooltip("Try again")
                    .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
            )
    }
}

impl Render for FilesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, dim, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let section = self.section;
        let header = v_flex()
            .gap_3()
            .pt_5()
            .pb_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child(section.title()),
            )
            .child(self.search_field(cx));
        let chips = self.chips(cx);
        let rows = &self.rows[section.source().index()];
        let error = match &rows.load {
            Load::Failed(error) => Some(Self::error_row(error, cx)),
            _ => None,
        };
        let body: AnyElement = if self.shown.is_empty() {
            match &rows.load {
                Load::Loading => state_block(None, "Loading…", fg, dim).into_any_element(),
                // The error row stands in for a page that never loaded.
                Load::Failed(_) if rows.files.is_empty() => div().into_any_element(),
                _ if self.chip != FilesChip::All || !self.query.is_empty() => {
                    state_block(Some("No matches"), "Try another search or filter.", fg, dim)
                        .into_any_element()
                }
                _ => {
                    let (title, line) = section.empty_state();
                    state_block(Some(title), line, fg, dim).into_any_element()
                }
            }
        } else {
            v_flex()
                .rounded_lg()
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .children(self.shown.iter().enumerate().map(|(position, &index)| {
                    file_row(&rows.files[index], &rows.meta[index], position > 0, cx)
                }))
                .into_any_element()
        };
        v_flex()
            .size_full()
            .text_sm()
            .text_color(fg)
            .child(centered(header))
            .child(
                // One scroll state per page, so switching pages starts at the top.
                v_flex()
                    .id(SharedString::from(format!("files-page-{section:?}")))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(centered(
                        v_flex()
                            .gap_3()
                            .pb_6()
                            .child(chips)
                            .children(error)
                            .child(body),
                    )),
            )
    }
}

/// `content` in the page's centered column.
fn centered(content: impl IntoElement) -> Div {
    h_flex()
        .w_full()
        .justify_center()
        .child(div().w_full().max_w(px(COLUMN_WIDTH)).px_6().child(content))
}

/// A centered empty or loading state: an optional bold title over a muted line.
fn state_block(title: Option<&'static str>, line: &'static str, fg: Hsla, dim: Hsla) -> Div {
    v_flex()
        .items_center()
        .gap_1()
        .py_16()
        .children(title.map(|title| {
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(fg)
                .child(title)
        }))
        .child(div().text_color(dim).child(line))
}

/// One file: type tile, bold name, and the "<owner> · Created <when>" line; opens the file's
/// permalink in the browser (a file without one is a plain row).
fn file_row(file: &File, meta: &SharedString, divided: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (icon, tile) = file_icon(file, cx);
    let row = h_flex()
        .id(SharedString::from(format!("files-row-{}", file.id)))
        .items_center()
        .gap_3()
        .px_3()
        .py_2()
        .when(divided, |row| row.border_t_1().border_color(theme.border))
        .child(
            div()
                .size(px(32.))
                .flex_none()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(tile)
                .child(
                    Icon::new(icon)
                        .text_color(crate::theme::avatar_foreground())
                        .size(px(18.)),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::BOLD)
                        .child(display_name(file).to_owned()),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(meta.clone()),
                ),
        );
    let link = file
        .permalink
        .clone()
        .or_else(|| file.url_private_download.clone())
        .or_else(|| file.url_private.clone());
    match link {
        Some(link) => {
            let hover = theme.accent;
            row.cursor_pointer()
                .hover(move |row| row.bg(hover))
                .on_click(move |_, _, cx| cx.open_url(&link))
        }
        None => row,
    }
}

/// The name Slack shows for a file: its title (a canvas's name, an upload's title), else its
/// file name, else its id.
fn display_name(file: &File) -> &str {
    [file.title.as_deref(), file.name.as_deref()]
        .into_iter()
        .flatten()
        .find(|name| !name.is_empty())
        .unwrap_or(file.id.as_str())
}

/// The rows a page lists, as indices into `files` (in order): the page's kind, then the
/// chip - Created by you is what `me` uploaded, Shared with you everyone else's - then the
/// search, `query` (already trimmed and lowercased) against the title or file name.
fn visible_files(
    files: &[File],
    section: FilesSection,
    chip: FilesChip,
    me: &UserId,
    query: &str,
) -> Vec<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, file)| section.holds(file))
        .filter(|(_, file)| {
            let mine = file.user.as_ref() == Some(me);
            match chip {
                FilesChip::All => true,
                FilesChip::CreatedByYou => mine,
                FilesChip::SharedWithYou => !mine,
            }
        })
        .filter(|(_, file)| {
            query.is_empty()
                || [file.title.as_deref(), file.name.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|name| name.to_lowercase().contains(query))
        })
        .map(|(index, _)| index)
        .collect()
}

/// Each row's muted line: "<owner> · Created <when>", the owner marked "(you)" for the
/// signed-in user; either half is left out when unknown.
fn metas(fixtures: &Fixtures, files: &[File]) -> Vec<SharedString> {
    files
        .iter()
        .map(|file| {
            let owner = file.user.as_ref().map(|user| {
                let name = fixtures.user_name(user);
                if *user == fixtures.self_user {
                    format!("{name} (you)")
                } else {
                    name.to_owned()
                }
            });
            let created = file
                .created
                .or(file.timestamp)
                .map(|secs| format!("Created {}", file_day(secs)));
            owner
                .into_iter()
                .chain(created)
                .collect::<Vec<_>>()
                .join(" · ")
                .into()
        })
        .collect()
}

/// The sample workspace's rows: every file its messages carry, newest first, each once,
/// owned and dated by its message (the fixture files carry neither), capped like a live page.
fn fixture_files(fixtures: &Fixtures) -> Vec<File> {
    let mut files: Vec<File> = fixtures
        .messages
        .values()
        .flatten()
        .flat_map(|message| {
            message.files.iter().map(move |file| File {
                user: file.user.clone().or_else(|| message.user.clone()),
                created: file.created.or_else(|| message.ts.secs()),
                ..file.clone()
            })
        })
        .collect();
    files.sort_by(|a, b| {
        b.created
            .cmp(&a.created)
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });
    let mut seen = HashSet::new();
    files.retain(|file| seen.insert(file.id.clone()));
    files.truncate(MAX_ROWS);
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "U0ME";

    fn file(id: &str, name: &str, filetype: &str, user: &str) -> File {
        File {
            id: id.into(),
            name: Some(name.to_owned()),
            filetype: Some(filetype.to_owned()),
            user: Some(user.into()),
            ..Default::default()
        }
    }

    /// The ids `visible_files` lists from a mixed set: a PDF and a list of yours, a canvas
    /// and an image (titled apart from its file name) of other people's.
    fn shown(section: FilesSection, chip: FilesChip, query: &str) -> Vec<String> {
        let files = [
            file("F1", "Q4-plan.pdf", "pdf", ME),
            file("F2", "Roadmap", "quip", "U0ALICE"),
            file("F3", "Launch tasks", "list", ME),
            File {
                title: Some("Dashboard mockup".to_owned()),
                ..file("F4", "dashboard.png", "png", "U0CARA")
            },
        ];
        visible_files(&files, section, chip, &ME.into(), query)
            .into_iter()
            .map(|index| files[index].id.to_string())
            .collect()
    }

    #[test]
    fn chips_split_the_rows_into_yours_and_everyone_elses() {
        use FilesChip::*;
        assert_eq!(shown(FilesSection::All, All, ""), ["F1", "F2", "F3", "F4"]);
        assert_eq!(shown(FilesSection::All, CreatedByYou, ""), ["F1", "F3"]);
        assert_eq!(shown(FilesSection::All, SharedWithYou, ""), ["F2", "F4"]);
    }

    #[test]
    fn pages_keep_their_kind_under_every_chip() {
        assert_eq!(shown(FilesSection::Canvases, FilesChip::All, ""), ["F2"]);
        assert!(shown(FilesSection::Canvases, FilesChip::CreatedByYou, "").is_empty());
        assert_eq!(
            shown(FilesSection::Lists, FilesChip::CreatedByYou, ""),
            ["F3"]
        );
        assert!(shown(FilesSection::Lists, FilesChip::SharedWithYou, "").is_empty());
    }

    #[test]
    fn search_matches_the_title_or_file_name_ignoring_case() {
        assert_eq!(shown(FilesSection::All, FilesChip::All, "plan"), ["F1"]);
        assert_eq!(shown(FilesSection::All, FilesChip::All, "roadmap"), ["F2"]);
        assert_eq!(shown(FilesSection::All, FilesChip::All, "mockup"), ["F4"]);
        assert_eq!(
            shown(FilesSection::All, FilesChip::All, "dashboard.png"),
            ["F4"]
        );
        // The search narrows the chip's rows, never widens them.
        assert_eq!(
            shown(FilesSection::All, FilesChip::SharedWithYou, "a"),
            ["F2", "F4"]
        );
        assert!(shown(FilesSection::All, FilesChip::All, "budget").is_empty());
    }
}
