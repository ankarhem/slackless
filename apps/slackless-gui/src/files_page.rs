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
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use slackless_core::api::{DeletedFile, FileCollection, FilesFilter};
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
    Starred,
    Deleted,
}

impl FilesSection {
    pub const ALL: [FilesSection; 5] = [
        Self::All,
        Self::Canvases,
        Self::Lists,
        Self::Starred,
        Self::Deleted,
    ];

    /// The list-column row's label and the page's title.
    pub fn title(self) -> &'static str {
        match self {
            Self::All => "All files",
            Self::Canvases => "Canvases",
            Self::Lists => "Lists",
            Self::Starred => "Starred",
            Self::Deleted => "Recently deleted",
        }
    }

    /// The list-column row's icon.
    pub fn icon(self) -> IconName {
        match self {
            Self::All => IconName::FileStack,
            Self::Canvases => IconName::StickyNote,
            Self::Lists => IconName::Table2,
            Self::Starred => IconName::Star,
            Self::Deleted => IconName::Trash,
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::All => "Search files",
            Self::Canvases => "Search canvases",
            Self::Lists => "Search lists",
            Self::Starred => "Search starred files",
            Self::Deleted => "Search deleted canvases & lists",
        }
    }

    /// Whether `file` belongs on this page: canvases are `quip` files, lists `list` files.
    fn holds(self, file: &File) -> bool {
        match self {
            Self::All | Self::Starred | Self::Deleted => true,
            Self::Canvases => file.filetype.as_deref() == Some("quip"),
            Self::Lists => file.filetype.as_deref() == Some("list"),
        }
    }

    fn source(self) -> Source {
        match self {
            Self::All | Self::Lists | Self::Starred | Self::Deleted => Source::Workspace,
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
            Self::Starred => (
                "No starred files yet",
                "Click the star on any file, canvas, or list to keep it handy here.",
            ),
            Self::Deleted => (
                "Nothing in trash",
                "Canvases and lists deleted in the last 24 hours can be restored here.",
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
            (Self::All, FilesSection::Starred) => "All Starred",
            (Self::All, FilesSection::Deleted) => "All Deleted",
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
    /// Fetch starred file IDs, custom file collections, and recently deleted items (`F-CORE-133`).
    LoadExtras,
    /// Save a file for Later (`saved.add` with `item_type=file`, `F-CORE-157`).
    SaveFile { file_id: String, name: String },
    /// Permanently delete a user-owned file/clip (`files.delete`, `F-CORE-130`).
    DeleteFile { file_id: String, name: String },
    /// Star/unstar a file or add/remove it from a collection (`files.favorites.*`, `F-CORE-133`).
    ToggleFavorite {
        file_id: String,
        collection_id: Option<String>,
        starred: bool,
    },
    /// Create a new custom file collection (`files.collections.create`, `F-CORE-133`).
    CreateCollection { name: String },
    /// Delete a custom file collection (`files.collections.delete`, `F-CORE-133`).
    DeleteCollection { collection_id: String },
    /// Restore a recently deleted canvas or list (`files.undelete`, `F-CORE-133`).
    RestoreFile { file_id: String, title: String },
    /// Preview a Slack canvas natively in the in-app Canvas reader (`F-COLLAB-125`).
    OpenCanvas { file_id: String },
    /// Preview a Slack List natively in the in-app editable Lists view.
    OpenList { file_id: String },
    /// Create a standalone canvas (`canvases.create`, `F-COLLAB-119`).
    CreateCanvas { title: String },
    /// Create a new Slack List from scratch (`lists.create`, `F-COLLAB-213`).
    CreateList { name: String },
    /// Pop out the Files browser into its own window (`F-COLLAB-127`, `F-SHELL-150`).
    PopOut,
}

/// Client-side sort order over the loaded files page (`F-CORE-133`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilesSort {
    Newest,
    Oldest,
    Name,
    Largest,
}

impl FilesSort {
    const ALL: [FilesSort; 4] = [Self::Newest, Self::Oldest, Self::Name, Self::Largest];

    fn label(self) -> &'static str {
        match self {
            Self::Newest => "Newest first",
            Self::Oldest => "Oldest first",
            Self::Name => "Name A\u{2013}Z",
            Self::Largest => "Largest first",
        }
    }
}

pub struct FilesPage {
    fixtures: Arc<Fixtures>,
    section: FilesSection,
    chip: FilesChip,
    sort: FilesSort,
    search: Entity<TextareaState>,
    /// The search, trimmed and lowercased; empty lists every row.
    query: String,
    /// Per [`Source`], its rows.
    rows: [Rows; 2],
    /// Indices into the shown source's rows that the page lists, recomputed when the rows,
    /// the page, the chip or the search change.
    shown: Vec<usize>,
    /// Starred file IDs (`files.favorites.list`, `F-CORE-133`).
    favorites: HashSet<String>,
    /// Custom file collections (`files.collections.list`, `F-CORE-133`).
    collections: Vec<FileCollection>,
    /// Active custom collection ID filter (`None` = all files in the section).
    active_collection: Option<String>,
    /// Recently deleted canvases & lists (`files.recentlyDeleted`, `F-CORE-133`).
    deleted: Vec<DeletedFile>,
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
                sort: FilesSort::Newest,
                search,
                query: String::new(),
                rows: Default::default(),
                shown: Vec::new(),
                favorites: HashSet::new(),
                collections: Vec::new(),
                active_collection: None,
                deleted: Vec::new(),
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
        if !self.fixtures.sample {
            cx.emit(FilesPageEvent::LoadExtras);
        }
        cx.notify();
    }

    /// Land starred IDs, custom file collections, and recently deleted canvases/lists (`F-CORE-133`).
    pub fn set_extras(
        &mut self,
        favorites: Vec<String>,
        collections: Vec<FileCollection>,
        deleted: Vec<DeletedFile>,
        cx: &mut Context<Self>,
    ) {
        self.favorites = favorites.into_iter().collect();
        self.collections = collections;
        if let Some(active) = &self.active_collection
            && !self.collections.iter().any(|c| &c.id == active)
        {
            self.active_collection = None;
        }
        self.deleted = deleted;
        self.refilter();
        cx.notify();
    }

    /// Optimistically toggle a file's star state locally before `files.favorites.*` lands.
    pub fn toggle_favorite_local(&mut self, file_id: &str, cx: &mut Context<Self>) -> bool {
        let now_starred = if self.favorites.contains(file_id) {
            self.favorites.remove(file_id);
            false
        } else {
            self.favorites.insert(file_id.to_owned());
            true
        };
        self.refilter();
        cx.notify();
        now_starred
    }

    /// Remove a restored file from the Deleted list immediately (`Event::FileRestored`).
    pub fn mark_restored(&mut self, file_id: &str, cx: &mut Context<Self>) {
        self.deleted.retain(|d| d.id != file_id);
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

    /// Remove a deleted file from all cached sources immediately (`Event::FileDeleted`).
    pub fn remove_file(&mut self, file_id: &str, cx: &mut Context<Self>) {
        for rows in &mut self.rows {
            rows.files.retain(|f| f.id.as_str() != file_id);
            rows.meta = metas(&self.fixtures, &rows.files);
        }
        self.refilter();
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
        let collection_ids: Option<HashSet<&str>> =
            self.active_collection.as_ref().and_then(|id| {
                self.collections
                    .iter()
                    .find(|c| &c.id == id)
                    .map(|c| c.file_ids.iter().map(String::as_str).collect())
            });
        let mut shown = visible_files(
            &rows.files,
            self.section,
            self.chip,
            &self.fixtures.self_user,
            &self.query,
        );
        if self.section == FilesSection::Starred {
            shown.retain(|&ix| self.favorites.contains(rows.files[ix].id.as_str()));
        }
        if let Some(ids) = collection_ids {
            shown.retain(|&ix| ids.contains(rows.files[ix].id.as_str()));
        }
        sort_visible_files(&rows.files, &mut shown, self.sort);
        self.shown = shown;
    }

    /// Slack's search box: a full-width field led by the magnifier.
    fn search_field(&self, cx: &Context<Self>) -> Div {
        crate::preferences::search_input_box(&self.search, cx)
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
        let (fg, dim, border, fill, hover) = {
            let theme = cx.theme();
            (
                theme.foreground,
                theme.muted_foreground,
                theme.border,
                theme.link,
                theme.accent,
            )
        };
        let section = self.section;
        let this_hdr = cx.weak_entity();
        let this_pop = cx.weak_entity();
        let show_new_canvas = matches!(section, FilesSection::All | FilesSection::Canvases);
        let show_new_list = matches!(section, FilesSection::All | FilesSection::Lists);
        let header = v_flex()
            .gap_3()
            .pt_5()
            .pb_3()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child(section.title()),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .when(show_new_canvas, |row| {
                                let page = this_hdr.clone();
                                row.child(
                                    Button::new("files-new-canvas")
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Plus)
                                        .label("New canvas")
                                        .on_click(move |_, window, cx| {
                                            open_new_canvas_dialog(page.clone(), window, cx);
                                        }),
                                )
                            })
                            .when(show_new_list, |row| {
                                let page = this_hdr.clone();
                                row.child(
                                    Button::new("files-new-list")
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Plus)
                                        .label("New list")
                                        .on_click(move |_, window, cx| {
                                            open_new_list_dialog(page.clone(), window, cx);
                                        }),
                                )
                            })
                            .child(
                                Button::new("files-popout")
                                    .outline()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::ExternalLink)
                                    .label("Pop out")
                                    .tooltip("Open Files in a new window")
                                    .on_click(move |_, _, cx| {
                                        let _ = this_pop.update(cx, |_, cx| {
                                            cx.emit(FilesPageEvent::PopOut);
                                        });
                                    }),
                            ),
                    ),
            )
            .child(self.search_field(cx));
        let chips = self.chips(cx);
        let current_sort = self.sort;
        let this = cx.weak_entity();
        let sort_menu = Button::new("files-sort-menu")
            .outline()
            .with_size(Size::XSmall)
            .icon(IconName::ArrowUpDown)
            .label(current_sort.label())
            .dropdown_menu(move |mut menu, _, _| {
                for choice in FilesSort::ALL {
                    let this = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new(choice.label())
                            .checked(choice == current_sort)
                            .on_click(move |_, _, cx| {
                                let _ = this.update(cx, |page, cx| {
                                    page.sort = choice;
                                    page.refilter();
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });
        let toolbar = h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(chips)
            .child(sort_menu);
        // Custom file collections strip (`files.collections.*`, `F-CORE-133`): lets users filter
        // by collection, delete a collection, or create a new one.
        let collections_bar = (section != FilesSection::Deleted
            && (!self.collections.is_empty() || !self.fixtures.sample))
            .then(|| {
                let on_fill = crate::theme::avatar_foreground();
                let active_col = self.active_collection.clone();
                let this_new = cx.weak_entity();
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1p5()
                    .child(div().text_xs().text_color(dim).child("Collections:"))
                    .children(self.collections.iter().enumerate().map(|(ix, col)| {
                        let cid = col.id.clone();
                        let cid_del = col.id.clone();
                        let active = active_col.as_deref() == Some(col.id.as_str());
                        let label = format!("{} ({})", col.name, col.file_ids.len());
                        h_flex()
                            .id(("files-collection", ix))
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .border_1()
                            .text_xs()
                            .cursor_pointer()
                            .when(active, |c| {
                                c.border_color(fill).bg(fill).text_color(on_fill)
                            })
                            .when(!active, |c| {
                                c.border_color(border).hover(move |c| c.bg(hover))
                            })
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active_collection =
                                    if this.active_collection.as_deref() == Some(cid.as_str()) {
                                        None
                                    } else {
                                        Some(cid.clone())
                                    };
                                this.refilter();
                                cx.notify();
                            }))
                            .when(active, |pill| {
                                pill.child(
                                    div()
                                        .id(("files-collection-del", ix))
                                        .pl_1()
                                        .cursor_pointer()
                                        .child("\u{2715}")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.active_collection = None;
                                            cx.emit(FilesPageEvent::DeleteCollection {
                                                collection_id: cid_del.clone(),
                                            });
                                            this.refilter();
                                            cx.notify();
                                        })),
                                )
                            })
                    }))
                    .child(
                        Button::new("files-collection-new")
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::Plus)
                            .label("New collection")
                            .on_click(move |_, window, cx| {
                                open_new_collection_dialog(this_new.clone(), window, cx);
                            }),
                    )
            });
        let rows = &self.rows[section.source().index()];
        let error = match &rows.load {
            Load::Failed(error) => Some(Self::error_row(error, cx)),
            _ => None,
        };
        let body: AnyElement = if section == FilesSection::Deleted {
            let matching_deleted: Vec<&DeletedFile> = self
                .deleted
                .iter()
                .filter(|d| self.query.is_empty() || d.title.to_lowercase().contains(&self.query))
                .collect();
            if matching_deleted.is_empty() {
                let (title, line) = section.empty_state();
                state_block(Some(title), line, fg, dim).into_any_element()
            } else {
                let this = cx.weak_entity();
                v_flex()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .children(matching_deleted.into_iter().enumerate().map(|(ix, item)| {
                        let fid = item.id.clone();
                        let title = item.title.clone();
                        let page_restore = this.clone();
                        let kind_label = match item.file_type.as_str() {
                            "quip" => "Canvas",
                            "list" => "List",
                            other if !other.is_empty() => other,
                            _ => "File",
                        };
                        let when = if item.date_delete > 0 {
                            format!("{kind_label} · Deleted {}", file_day(item.date_delete))
                        } else {
                            kind_label.to_owned()
                        };
                        h_flex()
                            .id(("files-deleted-row", ix))
                            .items_center()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .when(ix > 0, |r| r.border_t_1().border_color(border))
                            .child(
                                Icon::new(if item.file_type == "list" {
                                    IconName::Table2
                                } else {
                                    IconName::StickyNote
                                })
                                .size(px(18.))
                                .text_color(dim),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .truncate()
                                            .font_weight(FontWeight::BOLD)
                                            .child(item.title.clone()),
                                    )
                                    .child(div().text_xs().text_color(dim).child(when)),
                            )
                            .child(
                                Button::new(("files-restore", ix))
                                    .outline()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::RotateCcw)
                                    .label("Restore")
                                    .on_click(move |_, _, cx| {
                                        let _ = page_restore.update(cx, |_, cx| {
                                            cx.emit(FilesPageEvent::RestoreFile {
                                                file_id: fid.clone(),
                                                title: title.clone(),
                                            });
                                        });
                                    }),
                            )
                    }))
                    .into_any_element()
            }
        } else if self.shown.is_empty() {
            match &rows.load {
                Load::Loading => state_block(None, "Loading…", fg, dim).into_any_element(),
                // The error row stands in for a page that never loaded.
                Load::Failed(_) if rows.files.is_empty() => div().into_any_element(),
                _ if self.chip != FilesChip::All
                    || !self.query.is_empty()
                    || self.active_collection.is_some() =>
                {
                    state_block(Some("No matches"), "Try another search or filter.", fg, dim)
                        .into_any_element()
                }
                _ => {
                    let (title, line) = section.empty_state();
                    state_block(Some(title), line, fg, dim).into_any_element()
                }
            }
        } else {
            let this = cx.weak_entity();
            let self_user = self.fixtures.self_user.clone();
            let favorites = &self.favorites;
            let collections = &self.collections;
            v_flex()
                .rounded_lg()
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .children(self.shown.iter().enumerate().map(|(position, &index)| {
                    let file = &rows.files[index];
                    file_row(
                        file,
                        &rows.meta[index],
                        position > 0,
                        &self_user,
                        favorites.contains(file.id.as_str()),
                        collections,
                        this.clone(),
                        cx,
                    )
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
                            .when(section != FilesSection::Deleted, |col| col.child(toolbar))
                            .children(collections_bar)
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

impl FilesPage {
    /// Snapshot visible files for a popped-out Files browser window (`F-COLLAB-127`, `F-SHELL-150`).
    pub fn snapshot_for_popout(&self) -> Vec<(String, String, Option<String>)> {
        let rows = &self.rows[self.section.source().index()];
        self.shown
            .iter()
            .filter_map(|&ix| {
                let file = rows.files.get(ix)?;
                let meta = rows.meta.get(ix).map(|s| s.to_string()).unwrap_or_default();
                let url = file.permalink.clone().or_else(|| file.url_private.clone());
                Some((display_name(file).to_owned(), meta, url))
            })
            .collect()
    }

    /// Snapshot known Slack List `(file_id, title)` pairs for the "Add to list" message action (`F-COLLAB-231`).
    pub fn known_lists(&self) -> Vec<(String, String)> {
        self.rows[Source::Workspace.index()]
            .files
            .iter()
            .filter(|f| f.filetype.as_deref() == Some("list"))
            .map(|f| (f.id.to_string(), display_name(f).to_owned()))
            .collect()
    }
}

fn open_new_canvas_dialog(
    page: gpui_kit::gpui::WeakEntity<FilesPage>,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::gpui::Focusable as _;
    let title_state = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("Canvas title (e.g. Launch runbook)")
            .auto_grow(1, 1)
    });
    title_state.read(cx).focus_handle(cx).focus(window, cx);
    window.open_dialog(cx, move |dialog, _, cx| {
        let bg = cx.theme().background;
        let title_c = title_state.clone();
        let page_c = page.clone();
        dialog.title("Create standalone canvas").w(px(380.)).child(
            v_flex()
                .gap_3()
                .p_4()
                .child(crate::preferences::text_input_box(&title_state, false, cx).bg(bg))
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            crate::preferences::form_button(
                                Button::new("files-new-canvas-cancel")
                                    .outline()
                                    .label("Cancel"),
                                cx,
                            )
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            crate::preferences::form_button(
                                Button::new("files-new-canvas-create")
                                    .primary()
                                    .label("Create"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let title = title_c.read(cx).value().trim().to_owned();
                                let title = if title.is_empty() {
                                    "Untitled canvas".to_owned()
                                } else {
                                    title
                                };
                                window.close_dialog(cx);
                                let _ = page_c.update(cx, |_, cx| {
                                    cx.emit(FilesPageEvent::CreateCanvas { title });
                                });
                            }),
                        ),
                ),
        )
    });
}

fn open_new_list_dialog(
    page: gpui_kit::gpui::WeakEntity<FilesPage>,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::gpui::Focusable as _;
    let name_state = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("List name (e.g. Q4 release checklist)")
            .auto_grow(1, 1)
    });
    name_state.read(cx).focus_handle(cx).focus(window, cx);
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        let (bg, muted) = (theme.background, theme.muted_foreground);
        let name_c = name_state.clone();
        let page_c = page.clone();
        let templates = [
            ("Project tracker", "Project tracker"),
            ("Bug triage", "Bug triage"),
            ("Launch checklist", "Launch checklist"),
            ("Feedback tracker", "Customer feedback"),
        ];
        dialog.title("Create list").w(px(420.)).child(
            v_flex()
                .gap_3()
                .p_4()
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("Start from a template"),
                        )
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_1p5()
                                .children(templates.into_iter().map(|(label, preset)| {
                                    let ns = name_state.clone();
                                    Button::new(SharedString::from(format!(
                                        "files-list-tpl-{label}"
                                    )))
                                    .outline()
                                    .with_size(Size::XSmall)
                                    .label(label)
                                    .on_click(
                                        move |_, window, cx| {
                                            ns.update(cx, |st, cx| {
                                                st.set_value(preset.to_owned(), window, cx);
                                            });
                                        },
                                    )
                                })),
                        ),
                )
                .child(crate::preferences::text_input_box(&name_state, false, cx).bg(bg))
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            crate::preferences::form_button(
                                Button::new("files-new-list-cancel")
                                    .outline()
                                    .label("Cancel"),
                                cx,
                            )
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            crate::preferences::form_button(
                                Button::new("files-new-list-create")
                                    .primary()
                                    .label("Create"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let name = name_c.read(cx).value().trim().to_owned();
                                if name.is_empty() {
                                    return;
                                }
                                window.close_dialog(cx);
                                let _ = page_c.update(cx, |_, cx| {
                                    cx.emit(FilesPageEvent::CreateList { name });
                                });
                            }),
                        ),
                ),
        )
    });
}

fn open_new_collection_dialog(
    page: gpui_kit::gpui::WeakEntity<FilesPage>,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::gpui::Focusable as _;
    let name_state = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("Collection name (e.g. Design specs)")
            .auto_grow(1, 1)
    });
    name_state.read(cx).focus_handle(cx).focus(window, cx);
    window.open_dialog(cx, move |dialog, _, cx| {
        let bg = cx.theme().background;
        let name_c = name_state.clone();
        let page_c = page.clone();
        dialog.title("Create file collection").w(px(380.)).child(
            v_flex()
                .gap_3()
                .p_4()
                .child(crate::preferences::text_input_box(&name_state, false, cx).bg(bg))
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            crate::preferences::form_button(
                                Button::new("files-col-cancel").outline().label("Cancel"),
                                cx,
                            )
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            crate::preferences::form_button(
                                Button::new("files-col-create").primary().label("Create"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let name = name_c.read(cx).value().trim().to_owned();
                                if name.is_empty() {
                                    return;
                                }
                                window.close_dialog(cx);
                                let _ = page_c.update(cx, |_, cx| {
                                    cx.emit(FilesPageEvent::CreateCollection { name });
                                });
                            }),
                        ),
                ),
        )
    });
}

/// One file: type tile, bold name, and the "<owner> · Created <when>" line; opens canvases and
/// lists in their native readers (other files in the browser), offers star/collection toggles
/// (`F-CORE-133`), "Save for later" (`F-CORE-157`), and "Delete file" on user-owned files (`F-CORE-130`).
#[allow(clippy::too_many_arguments)]
fn file_row(
    file: &File,
    meta: &SharedString,
    divided: bool,
    self_user: &UserId,
    starred: bool,
    collections: &[FileCollection],
    page: gpui_kit::gpui::WeakEntity<FilesPage>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (icon, tile) = file_icon(file, cx);
    let file_id = file.id.to_string();
    let file_title = display_name(file).to_owned();
    let is_own = file.user.as_ref() == Some(self_user);
    let is_canvas = file.filetype.as_deref() == Some("quip");
    let is_list = file.filetype.as_deref() == Some("list");

    let page_star = page.clone();
    let fid_star = file_id.clone();
    let star_btn = Button::new(SharedString::from(format!("files-star-{file_id}")))
        .ghost()
        .with_size(Size::XSmall)
        .icon(IconName::Star)
        .tooltip(if starred { "Unstar file" } else { "Star file" })
        .when(starred, |b| b.text_color(gpui_kit::gpui::rgb(0xeab308)))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let _ = page_star.update(cx, |p, cx| {
                let now_starred = p.toggle_favorite_local(&fid_star, cx);
                cx.emit(FilesPageEvent::ToggleFavorite {
                    file_id: fid_star.clone(),
                    collection_id: None,
                    starred: now_starred,
                });
            });
        });

    let collection_btn = (!collections.is_empty()).then(|| {
        let cols: Vec<(String, String, bool)> = collections
            .iter()
            .map(|c| {
                (
                    c.id.clone(),
                    c.name.clone(),
                    c.file_ids.iter().any(|id| id == &file_id),
                )
            })
            .collect();
        let page_col = page.clone();
        let fid_col = file_id.clone();
        Button::new(SharedString::from(format!("files-col-menu-{file_id}")))
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::FolderPlus)
            .tooltip("Add to collection")
            .dropdown_menu(move |mut menu, _, _| {
                for (cid, name, in_col) in &cols {
                    let page_col = page_col.clone();
                    let fid_col = fid_col.clone();
                    let cid = cid.clone();
                    let in_col = *in_col;
                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(in_col).on_click(
                        move |_, _, cx| {
                            let _ = page_col.update(cx, |_, cx| {
                                cx.emit(FilesPageEvent::ToggleFavorite {
                                    file_id: fid_col.clone(),
                                    collection_id: Some(cid.clone()),
                                    starred: !in_col,
                                });
                            });
                        },
                    ));
                }
                menu
            })
    });

    let page_save = page.clone();
    let fid_save = file_id.clone();
    let title_save = file_title.clone();
    let save_btn = Button::new(SharedString::from(format!("files-save-{file_id}")))
        .ghost()
        .with_size(Size::XSmall)
        .icon(IconName::Bookmark)
        .tooltip("Save file for later")
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let _ = page_save.update(cx, |_, cx| {
                cx.emit(FilesPageEvent::SaveFile {
                    file_id: fid_save.clone(),
                    name: title_save.clone(),
                });
            });
        });
    let delete_btn = is_own.then(|| {
        let page_del = page.clone();
        let fid_del = file_id.clone();
        let title_del = file_title.clone();
        Button::new(SharedString::from(format!("files-del-{file_id}")))
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::Trash)
            .tooltip("Delete file")
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let _ = page_del.update(cx, |_, cx| {
                    cx.emit(FilesPageEvent::DeleteFile {
                        file_id: fid_del.clone(),
                        name: title_del.clone(),
                    });
                });
            })
    });
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
        )
        .child(star_btn)
        .children(collection_btn)
        .child(save_btn)
        .children(delete_btn);
    if is_canvas {
        let hover = theme.accent;
        let fid_canvas = file_id.clone();
        return row
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .on_click(move |_, _, cx| {
                let _ = page.update(cx, |_, cx| {
                    cx.emit(FilesPageEvent::OpenCanvas {
                        file_id: fid_canvas.clone(),
                    });
                });
            });
    }
    if is_list {
        let hover = theme.accent;
        let fid_list = file_id.clone();
        return row
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .on_click(move |_, _, cx| {
                let _ = page.update(cx, |_, cx| {
                    cx.emit(FilesPageEvent::OpenList {
                        file_id: fid_list.clone(),
                    });
                });
            });
    }
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

/// Sort filtered file indices by `sort` (newest, oldest, name A-Z, or largest first).
fn sort_visible_files(files: &[File], indices: &mut [usize], sort: FilesSort) {
    match sort {
        FilesSort::Newest => indices.sort_by(|&a, &b| {
            let ta = files[a].created.or(files[a].timestamp).unwrap_or(0);
            let tb = files[b].created.or(files[b].timestamp).unwrap_or(0);
            tb.cmp(&ta)
        }),
        FilesSort::Oldest => indices.sort_by(|&a, &b| {
            let ta = files[a].created.or(files[a].timestamp).unwrap_or(0);
            let tb = files[b].created.or(files[b].timestamp).unwrap_or(0);
            ta.cmp(&tb)
        }),
        FilesSort::Name => indices.sort_by(|&a, &b| {
            let na = files[a]
                .title
                .as_deref()
                .filter(|s| !s.is_empty())
                .or(files[a].name.as_deref())
                .unwrap_or(files[a].id.as_str());
            let nb = files[b]
                .title
                .as_deref()
                .filter(|s| !s.is_empty())
                .or(files[b].name.as_deref())
                .unwrap_or(files[b].id.as_str());
            na.to_lowercase().cmp(&nb.to_lowercase())
        }),
        FilesSort::Largest => {
            indices.sort_by(|&a, &b| files[b].size.unwrap_or(0).cmp(&files[a].size.unwrap_or(0)))
        }
    }
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

    #[test]
    fn sorts_visible_files_by_date_name_and_size() {
        let mut a = file("F1", "zebra.txt", "text", ME);
        a.created = Some(100);
        a.size = Some(500);
        let mut b = file("F2", "alpha.pdf", "pdf", ME);
        b.created = Some(300);
        b.size = Some(100);
        let mut c = file("F3", "middle.png", "png", ME);
        c.created = Some(200);
        c.size = Some(900);
        let files = vec![a, b, c];

        let run = |sort| {
            let mut ix = vec![0, 1, 2];
            sort_visible_files(&files, &mut ix, sort);
            ix.into_iter()
                .map(|i| files[i].id.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(run(FilesSort::Newest), ["F2", "F3", "F1"]);
        assert_eq!(run(FilesSort::Oldest), ["F1", "F3", "F2"]);
        assert_eq!(run(FilesSort::Name), ["F2", "F3", "F1"]);
        assert_eq!(run(FilesSort::Largest), ["F3", "F1", "F2"]);
    }
}
