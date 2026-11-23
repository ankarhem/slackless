//! Message composer, behaving like Slack's: a WYSIWYG draft (bold, italic, strikethrough,
//! code and links render in place, with no markers), lists, quotes and code blocks, and
//! completion menus for `@people`, `#channels`, `:emoji:` and `/commands`. Used by the main
//! column and the thread panel.
//!
//! The draft is edited in a gpui-kit textarea, but what it *is* lives in [`Editor`]: see
//! `editor/mod.rs` for that split and why. `composer/sync.rs` keeps the two in step: typing
//! reaches the model through `Composer::on_edits`, and everything the model changes on its
//! own goes back to the textarea through `Composer::push`.
//!
//! Enter sends and Shift+Enter breaks the line (Preferences can make Enter break the line
//! and Cmd+Enter send); inside a list, quote or code block Enter continues the block. A
//! sent draft is emitted as [`ComposerEvent::Submit`] with its `rich_text` block, or as
//! [`ComposerEvent::SlashCommand`] when it is a `/name args` line. The chevron beside Send
//! opens the schedule menu ([`ComposerEvent::Schedule`]). The root keeps per-conversation
//! drafts as mrkdwn through [`Composer::text`] / [`Composer::load_draft`].

mod keys;
mod menu;
mod menu_view;
mod sync;
mod tokens;
mod toolbar;
mod view;

use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEditsEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Size, WindowExt as _, h_flex, v_flex};
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, FontWeight,
    Image, ImageFormat, InteractiveElement as _, ParentElement as _, PathPromptOptions, Pixels,
    Point, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    px,
};
use slackless_core::api::{GifItem, MessageContent};
use slackless_core::types::Message;

use crate::drafts::SendLater;
use crate::editor::{Atom, Doc, Editor, from_mrkdwn, from_rich_text};
use crate::emoji::EmojiChoice;
use crate::fixtures::Fixtures;
use crate::slash::{self, Invocation};
use menu::Menu;
use tokens::Palette;

/// Slack's per-file upload cap (1 GiB, `F-CORE-123`).
pub const MAX_UPLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// A file or clipboard image staged in the composer, waiting to upload on send.
#[derive(Clone)]
pub struct PendingAttachment {
    pub filename: String,
    pub bytes: Vec<u8>,
    /// Thumbnail preview for pasted screenshots and attached image files.
    pub preview: Option<Arc<Image>>,
}

pub enum ComposerEvent {
    /// The message the user sent, never blank. The composer has already cleared itself.
    Submit(MessageContent),
    /// One or more attached files sent, with optional message text. The composer has cleared.
    SubmitWithFiles {
        content: Option<MessageContent>,
        files: Vec<PendingAttachment>,
    },
    /// The emoji button was clicked at this window position.
    OpenEmojiPicker(Point<Pixels>),
    /// `+` -> `GIF` was clicked: open the picker directly on the GIFs tab.
    OpenGifPicker(Point<Pixels>),
    /// A `/command` draft was sent (composer already cleared).
    SlashCommand(Invocation),
    /// The draft, as mrkdwn, was scheduled for later (composer already cleared).
    Schedule { text: String, when: SendLater },
    /// The user chose "Custom time..." in the Schedule Send menu (draft kept until confirmed).
    ScheduleCustom(String),
    /// Up was pressed in an empty draft: edit the user's last message here.
    EditLast,
    /// The user typed into the draft: tell the conversation they are typing (emitted at most
    /// every [`TYPING_EVERY`]).
    Typing,
    /// Edit mode was cancelled (Cancel button or Esc).
    Cancel,
}

/// How often Slack re-announces that the user is still typing; receivers drop the indicator
/// 5 s after the last announcement.
const TYPING_EVERY: Duration = Duration::from_secs(3);

/// Who Slack's notification bar under the box says is typing.
#[derive(Clone, PartialEq)]
pub enum TypingNames {
    One(SharedString),
    Two(SharedString, SharedString),
    Several,
}

impl TypingNames {
    /// The line for `names`, earliest typist first; `None` with nobody typing.
    pub fn of(names: &[SharedString]) -> Option<Self> {
        match names {
            [] => None,
            [one] => Some(Self::One(one.clone())),
            [one, other] => Some(Self::Two(one.clone(), other.clone())),
            _ => Some(Self::Several),
        }
    }

    /// Slack's wording (`TypingNames`), with the byte ranges of the names it sets in bold.
    fn line(&self) -> (String, Vec<Range<usize>>) {
        match self {
            Self::One(name) => {
                let bold = std::iter::once(0..name.len()).collect();
                (format!("{name} is typing"), bold)
            }
            Self::Two(name, other) => {
                let at = name.len() + " and ".len();
                let text = format!("{name} and {other} are typing");
                (text, vec![0..name.len(), at..at + other.len()])
            }
            Self::Several => ("Several people are typing".to_owned(), Vec::new()),
        }
    }
}

/// Where the thread composer's "Also send" checkbox posts the reply as well.
#[derive(Clone)]
pub enum AlsoSendTarget {
    /// "Also send to #name" (a lock instead of `#` for a private channel).
    Channel { name: SharedString, private: bool },
    /// "Also send as direct message".
    DirectMessage,
}

/// Builds the strip Slack draws inside the composer's frame above the formatting bar, its
/// context bar (the Slack Connect notice of a shared channel).
pub type ContextBar = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// How far the composer box overlaps its context bar; the bar adds this to its bottom padding
/// so its content stays clear.
pub const BAR_TUCK: f32 = 8.;

/// What the composer prompts with while empty.
pub enum Placeholder {
    /// Plain text: "Message #general", "Jot something down".
    Text(SharedString),
    /// A private channel's "Message 🔒name", the lock drawn as Slack's icon.
    Private(SharedString),
}

pub struct Composer {
    /// The textarea the draft is typed in.
    input: Entity<TextareaState>,
    /// What the draft is: its formatting, mentions, blocks and undo history.
    editor: Editor,
    fixtures: Arc<Fixtures>,
    /// The completion menu open at the caret.
    menu: Option<Menu>,
    /// Where Esc closed the menu: it stays closed while the same trigger is being typed.
    dismissed: Option<usize>,
    /// The URL field of the link prompt, shown while `link_open`.
    link_input: Entity<InputState>,
    link_open: bool,
    /// The theme colors the draft's formatting was last drawn with.
    palette: Option<Palette>,
    /// Whether the draft textarea was focused on the last frame (Live Preview reveals active
    /// markdown syntax only while focused).
    was_focused: bool,
    /// The textarea must be refilled from the editor on the next frame (that needs the
    /// window): a draft was loaded.
    is_stale: bool,
    /// A placeholder to apply on the next frame (setting it needs the window).
    pending_placeholder: Option<Placeholder>,
    /// The private channel the prompt names: drawn with its lock over the empty draft, as a
    /// textarea placeholder is plain text.
    private_placeholder: Option<SharedString>,
    /// The context bar over the formatting bar, while the conversation has one.
    context_bar: Option<ContextBar>,
    /// Preferences > Messages & media: Enter sends (Shift+Enter newline) when true; when
    /// false Enter inserts a newline and Cmd+Enter sends.
    enter_sends: bool,
    /// Whether the "Schedule for later" chevron is offered.
    scheduling: bool,
    /// The thread composer's "Also send" checkbox: its target and whether it is ticked.
    also_send: Option<(AlsoSendTarget, bool)>,
    /// Whether the top formatting toolbar is visible (toggled by the `Aa` button).
    show_toolbar: bool,
    /// Whether the bottom-left `+` attachments/shortcuts dropdown menu is open (`+` -> `×`).
    plus_open: bool,
    /// Timestamp of the last `+` toggle, so double-clicking `+` uploads directly like Slack.
    plus_last_click: Option<Instant>,
    /// Inline edit mode for an existing message: omits outer padding and notification bar,
    /// shows `+`, `Aa`, `☺` on the left and `Cancel` / `Save` on the right, and emits
    /// [`ComposerEvent::Cancel`] on Esc.
    edit_mode: bool,
    /// Files and pasted images staged for upload on send.
    attachments: Vec<PendingAttachment>,
    /// Friendly warning when a chosen/dropped file exceeds Slack's 1 GB upload cap (`F-CORE-123`).
    upload_warning: Option<String>,
    /// Native OS spellchecking (`F-SHELL-098`, `F-SHELL-099`).
    spellcheck: bool,
    spellcheck_lang: Option<&'static str>,
    misspellings: Vec<Range<usize>>,
    spell_guesses: Option<(Range<usize>, String, Vec<String>)>,
    /// Byte offset at the end of a word actively being typed; suppressed until word end or caret move.
    spell_typing_end: Option<usize>,
    /// A GIF selected from the GIFs picker, staged in the composer until send or removal.
    pending_gif: Option<GifItem>,
    /// Removed/replaced staged GIF URLs awaiting GPU texture eviction on the next render.
    stale_gif_urls: Vec<String>,
    /// Counter for naming pasted clipboard images (`image-1.png`, `image-2.png`, ...).
    next_paste_id: usize,
    /// Who the notification bar says is typing.
    typing: Option<TypingNames>,
    /// When [`ComposerEvent::Typing`] last went out.
    typing_sent: Option<Instant>,
    /// Bounded inline-code target captured on triple-click before `Textarea::select_line` runs.
    pending_code_click: Option<Range<usize>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

/// The message `doc` posts as: its `rich_text` block, with mrkdwn as the text fallback.
fn content(doc: &Doc) -> MessageContent {
    MessageContent {
        text: doc.to_mrkdwn(),
        blocks: vec![doc.to_rich_text().into()],
        attachments: Vec::new(),
    }
}

/// The message a mrkdwn draft posts as (a scheduled message is kept as its draft text).
pub fn content_of(draft: &str, fixtures: &Fixtures) -> MessageContent {
    content(&from_mrkdwn(draft, fixtures))
}

impl Composer {
    pub fn new(
        placeholder: SharedString,
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Composer> {
        Self::build(placeholder, fixtures, false, window, cx)
    }

    /// A WYSIWYG composer configured for editing a sent message in place.
    pub fn new_edit(
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Composer> {
        Self::build("Edit message".into(), fixtures, true, window, cx)
    }

    fn build(
        placeholder: SharedString,
        fixtures: Arc<Fixtures>,
        edit_mode: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Composer> {
        cx.new(|cx| {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(placeholder)
                    .auto_grow(1, 10)
            });
            let link_input =
                cx.new(|cx| InputState::new(window, cx).placeholder("Paste or type a link"));
            let subscriptions = vec![
                cx.subscribe_in(
                    &input,
                    window,
                    |this: &mut Composer, _, event: &InputEditsEvent, window, cx| {
                        this.on_edits(event, window, cx);
                    },
                ),
                cx.observe(&input, |this: &mut Composer, input, cx| {
                    this.on_moved(&input, cx);
                }),
            ];
            Composer {
                input,
                editor: Editor::default(),
                fixtures,
                menu: None,
                dismissed: None,
                link_input,
                link_open: false,
                palette: None,
                was_focused: false,
                is_stale: false,
                pending_placeholder: None,
                private_placeholder: None,
                context_bar: None,
                enter_sends: true,
                scheduling: !edit_mode,
                also_send: None,
                show_toolbar: true,
                plus_open: false,
                plus_last_click: None,
                edit_mode,
                attachments: Vec::new(),
                upload_warning: None,
                spellcheck: true,
                spellcheck_lang: None,
                misspellings: Vec::new(),
                spell_guesses: None,
                spell_typing_end: None,
                pending_gif: None,
                stale_gif_urls: Vec::new(),
                next_paste_id: 1,
                typing: None,
                typing_sent: None,
                pending_code_click: None,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Show who is typing in the notification bar under the box; `None` clears it.
    pub fn set_typing(&mut self, typing: Option<TypingNames>, cx: &mut Context<Self>) {
        if self.typing != typing {
            self.typing = typing;
            cx.notify();
        }
    }

    /// The user typed: emit [`ComposerEvent::Typing`], at most every [`TYPING_EVERY`].
    fn note_typing(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self
            .typing_sent
            .is_some_and(|sent| now - sent < TYPING_EVERY)
        {
            return;
        }
        self.typing_sent = Some(now);
        cx.emit(ComposerEvent::Typing);
    }

    /// Prompt with `placeholder` while empty ("Message #general"); applied on the next frame.
    /// Show `bar` inside the frame above the formatting bar, or remove it.
    pub fn set_context_bar(&mut self, bar: Option<ContextBar>, cx: &mut Context<Self>) {
        self.context_bar = bar;
        cx.notify();
    }

    pub fn set_placeholder(&mut self, placeholder: Placeholder, cx: &mut Context<Self>) {
        self.pending_placeholder = Some(placeholder);
        cx.notify();
    }

    /// Offer or hide the "Schedule for later" chevron (off in the thread reply composer).
    pub fn set_scheduling(&mut self, scheduling: bool, cx: &mut Context<Self>) {
        self.scheduling = scheduling;
        cx.notify();
    }

    /// Show Slack's "Also send to…" checkbox (unticked) under the draft, or hide it.
    pub fn set_also_send(&mut self, target: Option<AlsoSendTarget>, cx: &mut Context<Self>) {
        self.also_send = target.map(|target| (target, false));
        cx.notify();
    }

    /// Whether "Also send" was ticked for the reply just sent; like Slack, it unticks.
    pub fn take_also_send(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((_, ticked)) = &mut self.also_send else {
            return false;
        };
        cx.notify();
        std::mem::take(ticked)
    }

    /// Complete and name people, channels and emoji from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Switch between Enter-sends and Enter-inserts-a-newline (Cmd+Enter sends).
    pub fn set_enter_sends(&mut self, enter_sends: bool, cx: &mut Context<Self>) {
        self.enter_sends = enter_sends;
        self.input
            .update(cx, |state, cx| state.set_submit_on_enter(enter_sends, cx));
    }

    /// Enable/disable native OS spellchecking and pick its language (`F-SHELL-098`, `099`).
    pub fn set_spellcheck(
        &mut self,
        enabled: bool,
        language: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if self.spellcheck == enabled && self.spellcheck_lang == language {
            return;
        }
        self.spellcheck = enabled;
        self.spellcheck_lang = language;
        self.refresh_spellcheck();
        self.sync_presentation(cx);
        cx.notify();
    }

    /// The current draft serialized for local persistence (mrkdwn + `<u>` underline).
    pub fn text(&self) -> String {
        self.editor.doc().to_draft()
    }

    /// Replace the draft with `draft` (a saved per-conversation draft as mrkdwn, or empty).
    pub fn load_draft(&mut self, draft: &str, cx: &mut Context<Self>) {
        self.editor.load(from_mrkdwn(draft, &*self.fixtures));
        self.is_stale = true;
        self.link_open = false;
        if let Some(old) = self.pending_gif.take() {
            self.stale_gif_urls.push(old.preview_url);
        }
        self.changed(cx);
    }

    /// Load a sent message for editing: reads its `rich_text` blocks losslessly (falling back
    /// to mrkdwn), refills the textarea, and focuses at the end.
    pub fn load_message(&mut self, message: &Message, window: &mut Window, cx: &mut Context<Self>) {
        let doc = from_rich_text(&message.blocks, &*self.fixtures)
            .unwrap_or_else(|| from_mrkdwn(&message.text, &*self.fixtures));
        self.editor.load(doc);
        self.link_open = false;
        self.dismissed = None;
        self.was_focused = true;
        self.reset(window, cx);
        self.focus_input(window, cx);
        self.changed(cx);
    }

    /// Insert a picked emoji at the caret and give the draft focus back.
    pub fn insert_emoji(
        &mut self,
        choice: &EmojiChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = choice
            .glyph
            .clone()
            .unwrap_or_else(|| format!(":{}:", choice.name));
        let atom = Atom::Emoji(choice.name.as_str().into());
        self.editor
            .insert_atom(self.editor.selection(), atom, &shown);
        self.was_focused = true;
        self.focus_input(window, cx);
        self.push(window, cx);
        self.changed(cx);
    }

    /// Stage a GIF selected from the GIFs picker inside the composer (matching Slack's
    /// single-GIF staging card: picking another GIF replaces the previous one) and focus
    /// the draft so pressing Enter sends immediately or lets the user add text first.
    pub fn stage_gif(&mut self, gif: GifItem, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_mode {
            return;
        }
        if let Some(old) = self.pending_gif.replace(gif) {
            if self
                .pending_gif
                .as_ref()
                .is_none_or(|cur| cur.preview_url != old.preview_url)
            {
                self.stale_gif_urls.push(old.preview_url);
            }
        }
        self.was_focused = true;
        self.focus_input(window, cx);
        cx.notify();
    }

    pub(super) fn remove_gif(&mut self, cx: &mut Context<Self>) {
        if let Some(old) = self.pending_gif.take() {
            self.stale_gif_urls.push(old.preview_url);
            cx.notify();
        }
    }

    pub(crate) fn focus_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.read(cx).focus_handle(cx).focus(window, cx);
    }

    #[cfg(test)]
    pub(crate) fn token_ids_for_test(&self, cx: &App) -> Vec<(String, String)> {
        self.input
            .read(cx)
            .tokens()
            .iter()
            .map(|span| {
                (
                    span.token().id().to_string(),
                    span.token().text().to_string(),
                )
            })
            .collect()
    }

    /// Take the draft for sending: trimmed, and cleared from the composer. `None` when there
    /// is nothing to send.
    fn take(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Doc> {
        let doc = self.editor.doc().trimmed();
        if doc.is_blank() {
            return None;
        }
        self.editor.load(Doc::default());
        self.reset(window, cx);
        self.changed(cx);
        Some(doc)
    }

    /// Track `+` dropdown open state (`+` -> `×`) and trigger direct file upload on double-click,
    /// matching Slack's `"Tip: double click to upload"`.
    pub(super) fn on_plus_open_change(&mut self, open: bool, cx: &mut Context<Self>) {
        self.plus_open = open;
        let now = Instant::now();
        if !open
            && self
                .plus_last_click
                .is_some_and(|prev| now.duration_since(prev) <= Duration::from_millis(380))
        {
            self.plus_last_click = None;
            self.pick_files(cx);
        } else if open {
            self.plus_last_click = Some(now);
        }
        cx.notify();
    }

    /// Open Slack's `"Create a text snippet"` dialog (`+` -> `Text snippet` or `Cmd+Shift+Enter`),
    /// staging the snippet as an attached text/code file ready to upload.
    pub(super) fn open_text_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_mode {
            return;
        }
        let this = cx.entity().downgrade();
        let filename = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("snippet.txt")
                .auto_grow(1, 1)
        });
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Paste or type your snippet...")
                .auto_grow(6, 14)
        });
        body.read(cx).focus_handle(cx).focus(window, cx);
        const LANGUAGES: [(&str, &str); 9] = [
            ("Plain text", "txt"),
            ("Rust", "rs"),
            ("TypeScript", "ts"),
            ("Python", "py"),
            ("JSON", "json"),
            ("SQL", "sql"),
            ("Bash", "sh"),
            ("Markdown", "md"),
            ("Go", "go"),
        ];
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, bg, secondary, fg) = (
                theme.border,
                theme.muted_foreground,
                theme.background,
                theme.secondary,
                theme.foreground,
            );
            let (filename, body, this) = (filename.clone(), body.clone(), this.clone());
            let lang_pills =
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(LANGUAGES.iter().enumerate().map(|(ix, &(label, ext))| {
                        let filename = filename.clone();
                        div()
                            .id(("snippet-lang", ix))
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .border_1()
                            .border_color(border)
                            .bg(secondary)
                            .text_xs()
                            .text_color(fg)
                            .cursor_pointer()
                            .hover(|el| el.bg(theme.accent))
                            .child(label)
                            .on_click(move |_, window, cx| {
                                filename.update(cx, |state, cx| {
                                    let cur = state.value().trim().to_owned();
                                    let stem = cur
                                        .rsplit_once('.')
                                        .map(|(s, _)| s)
                                        .unwrap_or(cur.as_str());
                                    let stem = if stem.is_empty() { "snippet" } else { stem };
                                    state.set_value(format!("{stem}.{ext}"), window, cx);
                                });
                            })
                    }));
            dialog
                .title("Create a text or code snippet")
                .w(px(540.))
                .child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("Filename"),
                                )
                                .child(
                                    div()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(border)
                                        .bg(bg)
                                        .child(
                                            Textarea::new(&filename)
                                                .appearance(false)
                                                .bordered(false),
                                        ),
                                )
                                .child(lang_pills),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("Content"),
                                )
                                .child(
                                    div()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(border)
                                        .bg(bg)
                                        .child(
                                            Textarea::new(&body).appearance(false).bordered(false),
                                        ),
                                ),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("snippet-cancel")
                                        .outline()
                                        .with_size(Size::Small)
                                        .label("Cancel")
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("snippet-create")
                                        .primary()
                                        .with_size(Size::Small)
                                        .label("Create snippet")
                                        .on_click(move |_, window, cx| {
                                            let content = body.read(cx).value().to_string();
                                            if content.trim().is_empty() {
                                                return;
                                            }
                                            let raw_name =
                                                filename.read(cx).value().trim().to_owned();
                                            let name = if raw_name.is_empty() {
                                                "snippet.txt".to_owned()
                                            } else if raw_name.contains('.') {
                                                raw_name
                                            } else {
                                                format!("{raw_name}.txt")
                                            };
                                            if let Some(this) = this.upgrade() {
                                                this.update(cx, |composer, cx| {
                                                    composer.attachments.push(PendingAttachment {
                                                        filename: name,
                                                        bytes: content.into_bytes(),
                                                        preview: None,
                                                    });
                                                    cx.notify();
                                                });
                                            }
                                            window.close_dialog(cx);
                                        }),
                                ),
                        ),
                )
        });
    }

    /// Open the OS file picker and stage every chosen file as a pending attachment.
    pub(super) fn pick_files(&mut self, cx: &mut Context<Self>) {
        if self.edit_mode {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let _ = this.update(cx, |composer, cx| composer.attach_paths(paths, cx));
        })
        .detach();
    }

    /// Read on-disk files off the UI thread and stage them in `self.attachments`.
    pub(super) fn attach_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.edit_mode || paths.is_empty() {
            return;
        }
        self.upload_warning = None;
        cx.spawn(async move |this, cx| {
            let (loaded, rejected) = cx
                .background_executor()
                .spawn(async move {
                    let mut out = Vec::new();
                    let mut too_big = Vec::new();
                    for path in paths {
                        if !path.is_file() {
                            continue;
                        }
                        let filename = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("file")
                            .to_owned();
                        if let Ok(meta) = std::fs::metadata(&path) {
                            if meta.len() > MAX_UPLOAD_BYTES {
                                too_big.push(filename);
                                continue;
                            }
                        }
                        let Ok(bytes) = std::fs::read(&path) else {
                            continue;
                        };
                        if bytes.is_empty() {
                            continue;
                        }
                        let preview = image_format_for_ext(&path)
                            .map(|format| Arc::new(Image::from_bytes(format, bytes.clone())));
                        out.push(PendingAttachment {
                            filename,
                            bytes,
                            preview,
                        });
                    }
                    (out, too_big)
                })
                .await;
            if loaded.is_empty() && rejected.is_empty() {
                return;
            }
            let _ = this.update(cx, |composer, cx| {
                composer.attachments.extend(loaded);
                if !rejected.is_empty() {
                    composer.upload_warning = Some(format!(
                        "Skipped {} (exceeds Slack's 1 GB per-file upload limit).",
                        rejected.join(", ")
                    ));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Stage an image pasted from the clipboard (`Cmd+V`).
    pub(super) fn attach_clipboard_image(&mut self, image: Image, cx: &mut Context<Self>) {
        if self.edit_mode || image.bytes.is_empty() {
            return;
        }
        let ext = match image.format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
            ImageFormat::Gif => "gif",
            ImageFormat::Svg => "svg",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Tiff => "tiff",
            ImageFormat::Ico => "ico",
            ImageFormat::Pnm => "pnm",
        };
        let n = self.next_paste_id;
        self.next_paste_id += 1;
        let bytes = image.bytes.clone();
        self.attachments.push(PendingAttachment {
            filename: format!("image-{n}.{ext}"),
            bytes,
            preview: Some(Arc::new(image)),
        });
        cx.notify();
    }

    pub(super) fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.attachments.len() {
            self.attachments.remove(index);
            cx.notify();
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.edit_mode && !self.attachments.is_empty() {
            let files = std::mem::take(&mut self.attachments);
            let gif = self.pending_gif.take();
            let doc = self.take(window, cx);
            let msg = match (doc, gif) {
                (Some(doc), Some(gif)) => {
                    let mut c = content(&doc);
                    c.attachments.push(gif.to_attachment());
                    Some(c)
                }
                (Some(doc), None) => Some(content(&doc)),
                (None, Some(gif)) => Some(MessageContent {
                    text: String::new(),
                    blocks: Vec::new(),
                    attachments: vec![gif.to_attachment()],
                }),
                (None, None) => None,
            };
            cx.emit(ComposerEvent::SubmitWithFiles {
                content: msg,
                files,
            });
            cx.notify();
            return;
        }
        if !self.edit_mode {
            if let Some(gif) = self.pending_gif.take() {
                let doc = self.take(window, cx);
                let mut msg = match doc {
                    Some(doc) => content(&doc),
                    None => MessageContent::default(),
                };
                msg.attachments.push(gif.to_attachment());
                cx.emit(ComposerEvent::Submit(msg));
                cx.notify();
                return;
            }
        }
        let Some(doc) = self.take(window, cx) else {
            return;
        };
        if self.edit_mode {
            cx.emit(ComposerEvent::Submit(content(&doc)));
            return;
        }
        match slash::parse(doc.text()) {
            Some(invocation) => cx.emit(ComposerEvent::SlashCommand(invocation)),
            None => cx.emit(ComposerEvent::Submit(content(&doc))),
        }
    }

    /// Clear the draft and emit it as a scheduled message.
    fn schedule(&mut self, when: SendLater, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.take(window, cx) {
            let text = doc.to_mrkdwn();
            cx.emit(ComposerEvent::Schedule { text, when });
        }
    }

    /// Ask the root to open the Custom Schedule Send dialog for the current draft (does not
    /// clear the draft yet — root clears it once confirmed).
    fn schedule_custom(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.doc().to_mrkdwn();
        if !text.trim().is_empty() {
            cx.emit(ComposerEvent::ScheduleCustom(text));
        }
    }
}

fn image_format_for_ext(path: &std::path::Path) -> Option<ImageFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::Webp),
        "gif" => Some(ImageFormat::Gif),
        "bmp" => Some(ImageFormat::Bmp),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        _ => None,
    }
}

pub(super) fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use gpui_kit::gpui::{
        IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Render, TestAppContext,
        VisualTestContext,
    };

    struct ComposerHarness {
        composer: Entity<Composer>,
    }

    impl Render for ComposerHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.composer.clone())
        }
    }

    fn click_offset(
        composer: &Entity<Composer>,
        offset: usize,
        click_count: usize,
        cx: &mut VisualTestContext,
    ) {
        let pos = composer.read_with(cx, |composer, cx| {
            let b = composer
                .input
                .read(cx)
                .range_to_bounds(&(offset..offset + 1))
                .unwrap_or_else(|| panic!("no bounds for offset {offset}"));
            b.center()
        });
        cx.simulate_event(MouseDownEvent {
            position: pos,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position: pos,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn selected_slice(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> String {
        composer.read_with(cx, |composer, cx| {
            let state = composer.input.read(cx);
            let range = state.selected_range();
            state.text().slice(range).to_string()
        })
    }

    #[gpui::test]
    fn composer_multi_click_is_bounded_in_inline_code_and_code_blocks(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let mrkdwn = "see `order.customerType: \"Org\"` ok\n```line 1\nline 2```";
        let expected_text = "see `order.customerType: \"Org\"` ok\n```\nline 1\nline 2\n```";
        let slot = Rc::new(RefCell::new(None));
        let (root, cx) = cx.add_window_view(|window, cx| {
            let fixtures = Arc::new(crate::fixtures::load());
            let composer = Composer::new("Message".into(), fixtures, window, cx);
            composer.update(cx, |composer, cx| {
                composer.load_draft(mrkdwn, cx);
            });
            *slot.borrow_mut() = Some(composer.clone());
            let view = cx.new(|_| ComposerHarness { composer });
            gpui_kit::base::Root::new(view, window, cx)
        });
        let composer = slot.borrow_mut().take().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });

        let text = composer.read_with(cx, |c, _| c.editor.doc().text().to_owned());
        assert_eq!(text, expected_text);

        // Single, double, and triple click inside inline code (`customerType`).
        let word_ix = text.find("customerType").unwrap() + 2;
        click_offset(&composer, word_ix, 1, cx);
        click_offset(&composer, word_ix, 2, cx);
        assert_eq!(selected_slice(&composer, cx), "customerType");
        click_offset(&composer, word_ix, 3, cx);
        assert_eq!(selected_slice(&composer, cx), "order.customerType: \"Org\"");

        // Triple click on prose on the same line still selects the entire line.
        let prose_ix = text.find("see").unwrap() + 1;
        click_offset(&composer, prose_ix, 1, cx);
        click_offset(&composer, prose_ix, 2, cx);
        click_offset(&composer, prose_ix, 3, cx);
        assert_eq!(
            selected_slice(&composer, cx),
            "see `order.customerType: \"Org\"` ok"
        );

        // Double and triple click inside a multi-line fenced code block.
        let l2_ix = text.find("line 2").unwrap() + 1;
        click_offset(&composer, l2_ix, 1, cx);
        click_offset(&composer, l2_ix, 2, cx);
        assert_eq!(selected_slice(&composer, cx), "line");
        click_offset(&composer, l2_ix, 3, cx);
        assert_eq!(selected_slice(&composer, cx), "line 1\nline 2");
        drop(root);
    }
}
