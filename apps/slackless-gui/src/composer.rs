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
    Point, RenderImage, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Task, Window, div, px,
};
use slackless_core::api::{GifItem, MessageContent};
use slackless_core::huddle::{
    AudioClipRecorder, ClipVideoSource, VideoClipRecorder, finish_audio_clip,
};
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

/// Recorded video clip awaiting thumbnail selection and confirmation before attaching (`F-COLLAB-099`).
pub(super) struct ClipReview {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub duration_ms: u64,
    pub thumbs: Vec<Arc<RenderImage>>,
    pub selected_thumb: usize,
}

/// An active native audio or video clip recording session inside the composer (`F-CORE-127`, `128`, `131`).
pub(super) enum ActiveClipRecorder {
    Audio(AudioClipRecorder),
    Video {
        recorder: VideoClipRecorder,
        is_screen: bool,
    },
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
    /// When `true` (default), Live Preview hides markdown delimiters away from the caret (`F-CORE-079`).
    /// When `false` (Markup mode, `F-CORE-080`), formatting characters stay visible while typing.
    format_markup: bool,
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
    /// Active native audio/video clip recorder (`F-CORE-127`, `F-CORE-128`, `F-CORE-131`).
    pub(super) clip_recorder: Option<ActiveClipRecorder>,
    pub(super) clip_preview: Option<Arc<RenderImage>>,
    pub(super) clip_review: Option<ClipReview>,
    pub(super) clip_stopping: bool,
    clip_ticker: Option<Task<()>>,
    /// Measured outer width of the composer frame in logical pixels, used to collapse
    /// overflowing toolbar and action-row controls into a `"More options"` menu.
    pub(super) box_width: Option<f32>,
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
                format_markup: true,
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
                clip_recorder: None,
                clip_preview: None,
                clip_review: None,
                clip_stopping: false,
                clip_ticker: None,
                box_width: None,
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

    /// Toggle Live Preview Markdown autoformat (`true`, `F-CORE-079`) vs raw Markup mode
    /// (`false`, `F-CORE-080`, always keeping formatting delimiters visible).
    pub fn set_format_markup(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.format_markup != enabled {
            self.format_markup = enabled;
            self.sync_presentation(cx);
            cx.notify();
        }
    }

    fn clear_clip_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(old) = self.clip_preview.take() {
            cx.drop_image(old, None);
        }
    }

    fn clear_clip_review(&mut self, cx: &mut Context<Self>) -> Option<ClipReview> {
        let review = self.clip_review.take()?;
        for thumb in &review.thumbs {
            cx.drop_image(Arc::clone(thumb), None);
        }
        Some(review)
    }

    /// Start recording a native microphone audio clip (`F-CORE-127`, `F-CORE-131`).
    pub fn start_audio_clip(&mut self, cx: &mut Context<Self>) {
        if self.clip_stopping {
            return;
        }
        if self.clip_recorder.is_some() {
            self.stop_and_attach_clip(cx);
            return;
        }
        self.upload_warning = None;
        match AudioClipRecorder::start() {
            Ok(recorder) => {
                self.clip_recorder = Some(ActiveClipRecorder::Audio(recorder));
                self.clear_clip_preview(cx);
                self.spawn_clip_ticker(cx);
                cx.notify();
            }
            Err(err) => {
                self.upload_warning = Some(format!("Audio clip error: {err}"));
                cx.notify();
            }
        }
    }

    /// Start recording a native camera or screen video clip (`F-CORE-128`, `F-CORE-131`).
    pub fn start_video_clip(&mut self, source: ClipVideoSource, cx: &mut Context<Self>) {
        if self.clip_stopping {
            return;
        }
        if self.clip_recorder.is_some() {
            self.stop_and_attach_clip(cx);
            return;
        }
        self.upload_warning = None;
        let is_screen = matches!(source, ClipVideoSource::Screen(_));
        match VideoClipRecorder::start(source) {
            Ok(recorder) => {
                self.clip_recorder = Some(ActiveClipRecorder::Video {
                    recorder,
                    is_screen,
                });
                self.clear_clip_preview(cx);
                self.spawn_clip_ticker(cx);
                cx.notify();
            }
            Err(err) => {
                self.upload_warning = Some(format!("Video clip error: {err}"));
                cx.notify();
            }
        }
    }

    /// Discard an active clip recording without attaching it.
    pub fn cancel_clip(&mut self, cx: &mut Context<Self>) {
        self.clip_ticker = None;
        self.clear_clip_preview(cx);
        self.clip_stopping = false;
        if let Some(active) = self.clip_recorder.take() {
            match active {
                ActiveClipRecorder::Audio(r) => r.cancel(),
                ActiveClipRecorder::Video { recorder, .. } => recorder.cancel(),
            }
        }
        cx.notify();
    }

    /// Stop the active clip recorder, encode off the UI thread, and stage as a composer attachment.
    pub fn stop_and_attach_clip(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.clip_recorder.take() else {
            return;
        };
        self.clip_ticker = None;
        self.clear_clip_preview(cx);
        self.clip_stopping = true;
        cx.notify();

        match active {
            ActiveClipRecorder::Audio(recorder) => match recorder.stop_to_pcm() {
                Ok((pcm, elapsed_ms)) => {
                    cx.spawn(async move |this, cx| {
                        let res = cx
                            .background_executor()
                            .spawn(async move { finish_audio_clip(pcm, elapsed_ms) })
                            .await;
                        let _ = this.update(cx, |composer, cx| {
                            composer.clip_stopping = false;
                            match res {
                                Ok(clip) => {
                                    composer.attachments.push(PendingAttachment {
                                        filename: clip.filename,
                                        bytes: clip.bytes,
                                        preview: None,
                                    });
                                }
                                Err(err) => {
                                    composer.upload_warning =
                                        Some(format!("Audio clip encoding failed: {err}"));
                                }
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                }
                Err(err) => {
                    self.clip_stopping = false;
                    self.upload_warning = Some(format!("Audio clip error: {err}"));
                    cx.notify();
                }
            },
            ActiveClipRecorder::Video { recorder, .. } => {
                cx.spawn(async move |this, cx| {
                    let res = cx
                        .background_executor()
                        .spawn(async move { recorder.stop() })
                        .await;
                    let _ = this.update(cx, |composer, cx| {
                        composer.clip_stopping = false;
                        match res {
                            Ok(clip) => {
                                let thumbs: Vec<Arc<RenderImage>> = clip
                                    .thumbnail_frames
                                    .iter()
                                    .filter_map(crate::huddle_dock::frame_to_render_image)
                                    .collect();
                                if thumbs.is_empty() {
                                    composer.attachments.push(PendingAttachment {
                                        filename: clip.filename,
                                        bytes: clip.bytes,
                                        preview: None,
                                    });
                                } else {
                                    let _ = composer.clear_clip_review(cx);
                                    composer.clip_review = Some(ClipReview {
                                        filename: clip.filename,
                                        bytes: clip.bytes,
                                        duration_ms: clip.duration_ms,
                                        thumbs,
                                        selected_thumb: 0,
                                    });
                                }
                            }
                            Err(err) => {
                                composer.upload_warning =
                                    Some(format!("Video clip encoding failed: {err}"));
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    /// Confirm the reviewed video clip (keeping the chosen thumbnail) and attach it (`F-COLLAB-099`).
    pub fn confirm_clip_review(&mut self, cx: &mut Context<Self>) {
        if let Some(review) = self.clear_clip_review(cx) {
            self.attachments.push(PendingAttachment {
                filename: review.filename,
                bytes: review.bytes,
                preview: None,
            });
            cx.notify();
        }
    }

    /// Discard the pending video clip review without attaching.
    pub fn discard_clip_review(&mut self, cx: &mut Context<Self>) {
        let _ = self.clear_clip_review(cx);
        cx.notify();
    }

    fn spawn_clip_ticker(&mut self, cx: &mut Context<Self>) {
        self.clip_ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let keep_going = this
                    .update(cx, |composer, cx| {
                        let Some(active) = composer.clip_recorder.as_ref() else {
                            return false;
                        };
                        let elapsed = match active {
                            ActiveClipRecorder::Audio(rec) => rec.elapsed_ms(),
                            ActiveClipRecorder::Video { recorder, .. } => recorder.elapsed_ms(),
                        };
                        // Enforce Slack's 5-minute clip cap (`F-COLLAB-107`).
                        if elapsed >= slackless_core::huddle::MAX_CLIP_DURATION_MS {
                            composer.stop_and_attach_clip(cx);
                            return false;
                        }
                        if let ActiveClipRecorder::Video { recorder, .. } = active
                            && let Some(vf) = recorder.latest_preview()
                            && let Some(img) = crate::huddle_dock::frame_into_render_image(vf)
                            && let Some(old) = composer.clip_preview.replace(img)
                        {
                            cx.drop_image(old, None);
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
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
        if let Some(old) = self.pending_gif.replace(gif)
            && self
                .pending_gif
                .as_ref()
                .is_none_or(|cur| cur.preview_url != old.preview_url)
        {
            self.stale_gif_urls.push(old.preview_url);
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
                        if let Ok(meta) = std::fs::metadata(&path)
                            && meta.len() > MAX_UPLOAD_BYTES
                        {
                            too_big.push(filename);
                            continue;
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
        if !self.edit_mode
            && let Some(gif) = self.pending_gif.take()
        {
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
    crate::preferences::format_bytes(bytes as u64)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use gpui_kit::gpui::{
        ClipboardItem, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Render,
        TestAppContext, VisualTestContext,
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

        // Scoped SelectAll: first Cmd+A inside a fenced code block selects just the block,
        // second Cmd+A selects the entire draft.
        click_offset(&composer, l2_ix, 1, cx);
        cx.dispatch_action(gpui_kit::component::input::SelectAll);
        cx.run_until_parked();
        assert_eq!(selected_slice(&composer, cx), "line 1\nline 2");
        cx.dispatch_action(gpui_kit::component::input::SelectAll);
        cx.run_until_parked();
        assert_eq!(selected_slice(&composer, cx), expected_text);

        // Scoped SelectAll inside inline code: first selects inner text, second selects all.
        click_offset(&composer, word_ix, 1, cx);
        cx.dispatch_action(gpui_kit::component::input::SelectAll);
        cx.run_until_parked();
        assert_eq!(selected_slice(&composer, cx), "order.customerType: \"Org\"");
        cx.dispatch_action(gpui_kit::component::input::SelectAll);
        cx.run_until_parked();
        assert_eq!(selected_slice(&composer, cx), expected_text);
        drop(root);
    }

    #[gpui::test]
    fn composer_keys_paste_escape_up_auto_fence_and_typed_emoji_state_machines(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::component::input::{Enter, Escape, MoveUp};

        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });

        let submitted: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let slash_cmds: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let edit_last_count: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
        let cancel_count: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));

        struct SpyHost {
            composer: Entity<Composer>,
            _sub: Subscription,
        }
        impl Render for SpyHost {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.composer.clone())
            }
        }

        let slot = Rc::new(RefCell::new(None));
        let slot_c = slot.clone();
        let sub_c = submitted.clone();
        let slash_c = slash_cmds.clone();
        let edit_c = edit_last_count.clone();
        let cancel_c = cancel_count.clone();

        let (root, cx) = cx.add_window_view(move |window, cx| {
            let fixtures = Arc::new(crate::fixtures::load());
            let composer = Composer::new("Message".into(), fixtures, window, cx);
            *slot_c.borrow_mut() = Some(composer.clone());
            let view = cx.new(|cx| {
                let sub = cx.subscribe(&composer, move |_, _, ev: &ComposerEvent, _| match ev {
                    ComposerEvent::Submit(c) => sub_c.borrow_mut().push(c.text.clone()),
                    ComposerEvent::SlashCommand(inv) => slash_c.borrow_mut().push(inv.name.clone()),
                    ComposerEvent::EditLast => *edit_c.borrow_mut() += 1,
                    ComposerEvent::Cancel => *cancel_c.borrow_mut() += 1,
                    _ => {}
                });
                SpyHost {
                    composer,
                    _sub: sub,
                }
            });
            gpui_kit::base::Root::new(view, window, cx)
        });
        let composer = slot.borrow_mut().take().unwrap();
        cx.run_until_parked();

        // 1. on_up on empty composer emits EditLast; non-empty draft does not.
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| c.on_up(&MoveUp, window, cx));
        });
        assert_eq!(*edit_last_count.borrow(), 1);
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.editor.replace_all("draft text");
                c.on_up(&MoveUp, window, cx);
            });
        });
        assert_eq!(*edit_last_count.borrow(), 1);

        // 2. on_enter: enter_sends = true submits; Shift+Enter does not; enter_sends = false does not; Cmd+Enter submits.
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.editor.replace_all("hello");
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: true,
                    },
                    window,
                    cx,
                );
            });
        });
        assert!(submitted.borrow().is_empty(), "Shift+Enter must not submit");

        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.set_enter_sends(false, cx);
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
            });
        });
        assert!(
            submitted.borrow().is_empty(),
            "Enter must not submit when enter_sends = false"
        );

        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.on_enter(
                    &Enter {
                        secondary: true,
                        shift: false,
                    },
                    window,
                    cx,
                );
            });
        });
        assert_eq!(submitted.borrow().as_slice(), ["hello"]);

        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.set_enter_sends(true, cx);
                c.editor.replace_all("second");
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
            });
        });
        assert_eq!(submitted.borrow().as_slice(), ["hello", "second"]);

        // 3. Menu pick on partial slash command vs submitting a full typed slash command.
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.editor.replace_all("/sh");
                c.changed(cx);
                assert!(c.menu.is_some());
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
                assert_eq!(c.editor.doc().text(), "/shrug ");
            });
        });
        assert!(slash_cmds.borrow().is_empty());

        // Full typed command submits SlashCommand instead of re-completing.
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.editor.replace_all("/away");
                c.changed(cx);
                assert!(c.menu.is_some());
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
            });
        });
        assert_eq!(slash_cmds.borrow().as_slice(), ["away"]);

        // 4. on_escape sticky menu dismiss: stays dismissed while typing same trigger.
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.editor.replace_all("hi @");
                c.changed(cx);
                assert!(c.menu.is_some());
                c.on_escape(&Escape, window, cx);
                assert!(c.menu.is_none());
                assert_eq!(c.dismissed, Some(3));

                // Continuing the same `@` trigger keeps menu dismissed.
                c.editor.insert_text("a");
                c.changed(cx);
                assert!(c.menu.is_none());

                // Moving past space clears dismissal.
                c.editor.insert_text(" ");
                c.changed(cx);
                assert_eq!(c.dismissed, None);
            });
        });

        // 5. on_paste: URL over selection wraps as [text](url); empty selection or non-URL returns false.
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.editor.replace_all("read docs now");
                c.editor.select(5..9);
                let url_clip = ClipboardItem::new_string("https://example.com".into());
                assert!(c.on_paste(&url_clip, window, cx));
                assert_eq!(
                    c.editor.doc().text(),
                    "read [docs](https://example.com) now"
                );

                // Empty selection -> false
                assert!(!c.on_paste(&url_clip, window, cx));

                // Non-URL over selection -> false
                c.editor.select(0..4);
                let plain_clip = ClipboardItem::new_string("not a url".into());
                assert!(!c.on_paste(&plain_clip, window, cx));
            });
        });

        // 6. auto_open_code_block opens on opening fence and does NOT open on closing fence.
        cx.update(|_, cx| {
            composer.update(cx, |c, _| {
                c.editor.replace_all("```");
                assert!(c.auto_open_code_block());
                assert_eq!(c.editor.doc().text(), "```\n\n```");
                assert_eq!(c.editor.selection(), 4..4);

                // Closing an already-opened fence must not auto-open again.
                c.editor.replace_all("```\ncode\n```");
                assert!(!c.auto_open_code_block());
                assert_eq!(c.editor.doc().text(), "```\ncode\n```");
            });
        });

        // 7. convert_typed_emoji converts `:smile:` in prose, skips inline code, fenced code, and link URLs.
        cx.update(|_, cx| {
            composer.update(cx, |c, _| {
                c.editor.replace_all("hi :smile:");
                assert!(c.convert_typed_emoji());
                assert_eq!(c.editor.doc().atoms().len(), 1);
                assert_eq!(c.editor.doc().to_mrkdwn(), "hi :smile:");

                // Skipped inside inline code
                c.editor.replace_all("`:smile:`");
                c.editor.select(8..8);
                assert!(!c.convert_typed_emoji());
                assert!(c.editor.doc().atoms().is_empty());

                // Skipped inside fenced code block
                c.editor.replace_all("```\n:smile:\n```");
                c.editor.select(11..11);
                assert!(!c.convert_typed_emoji());
                assert!(c.editor.doc().atoms().is_empty());

                // Skipped inside link URL
                c.editor.replace_all("[lbl](https://x.dev/:smile:)");
                c.editor.select(27..27);
                assert!(!c.convert_typed_emoji());
                assert!(c.editor.doc().atoms().is_empty());
            });
        });

        // 8. Edit mode Escape emits Cancel.
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.edit_mode = true;
                c.on_escape(&Escape, window, cx);
            });
        });
        assert_eq!(*cancel_count.borrow(), 1);

        drop(root);
    }

    fn key_event(key: &str, shift: bool, alt: bool, cx: &App) -> gpui_kit::gpui::KeyDownEvent {
        let mut modifiers = Modifiers::secondary_key(cx);
        modifiers.shift = shift;
        modifiers.alt = alt;
        gpui_kit::gpui::KeyDownEvent {
            keystroke: gpui_kit::gpui::Keystroke {
                modifiers,
                key: key.to_owned(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[gpui::test]
    fn composer_formatting_shortcuts_link_prompt_and_undo_redo(cx: &mut TestAppContext) {
        use gpui_kit::component::input::{Enter, Escape, Redo, Undo};

        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let slot = Rc::new(RefCell::new(None));
        let slot_c = slot.clone();
        let (root, cx) = cx.add_window_view(move |window, cx| {
            let fixtures = Arc::new(crate::fixtures::load());
            let composer = Composer::new("Message".into(), fixtures, window, cx);
            *slot_c.borrow_mut() = Some(composer.clone());
            let view = cx.new(|_| ComposerHarness { composer });
            gpui_kit::base::Root::new(view, window, cx)
        });
        let composer = slot.borrow_mut().take().unwrap();
        cx.run_until_parked();

        // Inline styles: Cmd+B, Cmd+I, Cmd+U, Cmd+Shift+X, Cmd+Shift+C, Cmd+\
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.editor.replace_all("hello world");
                c.editor.select(0..5);
                let ev = key_event("b", false, false, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "*hello* world");
                assert_eq!(c.editor.selection(), 1..6);

                let ev = key_event("i", false, false, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "*_hello_* world");

                let ev = key_event("u", false, false, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "*_<u>hello</u>_* world");

                let ev = key_event("x", true, false, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "*_<u>~hello~</u>_* world");

                // Cmd+\ clears all inline styles
                let ev = key_event("\\", false, false, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "hello world");

                // Cmd+Shift+C wraps in inline code
                c.editor.select(6..11);
                let ev = key_event("c", true, false, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "hello `world`");

                // Undo restores before inline code, redo reapplies it
                c.on_undo(&Undo, window, cx);
                assert_eq!(c.editor.doc().text(), "hello world");
                c.on_redo(&Redo, window, cx);
                assert_eq!(c.editor.doc().text(), "hello `world`");
            });
        });

        // Link prompt (Cmd+Shift+U): open -> confirm bare domain -> unlink -> open -> Esc cancel
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.editor.replace_all("read docs");
                c.editor.select(5..9);
                let ev = key_event("u", true, false, cx);
                c.on_key(&ev, window, cx);
                assert!(c.link_open, "Cmd+Shift+U opens link prompt");

                c.link_input
                    .update(cx, |st, cx| st.set_value("slack.com/guide", window, cx));
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
                assert!(!c.link_open);
                assert_eq!(
                    c.editor.doc().text(),
                    "read [docs](https://slack.com/guide)"
                );

                // Cmd+Shift+U on an existing link unlinks it
                c.editor.select(7..9);
                let ev = key_event("u", true, false, cx);
                c.on_key(&ev, window, cx);
                assert!(!c.link_open);
                assert_eq!(c.editor.doc().text(), "read docs");

                // Open link prompt again and cancel with Esc
                c.editor.select(5..9);
                let ev = key_event("u", true, false, cx);
                c.on_key(&ev, window, cx);
                assert!(c.link_open);
                c.link_input
                    .update(cx, |st, cx| st.set_value("ignored.dev", window, cx));
                c.on_escape(&Escape, window, cx);
                assert!(!c.link_open);
                assert_eq!(c.editor.doc().text(), "read docs");
            });
        });

        // Block shortcuts: Cmd+Shift+7 (ordered), Cmd+Shift+8 (bullet), Cmd+Shift+9 (quote), Cmd+Alt+Shift+C (code block)
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.editor.replace_all("item one");
                c.editor.select(0..8);
                let ev = key_event("7", true, false, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "1. item one");

                let ev = key_event("8", true, false, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "- item one");

                let ev = key_event("9", true, false, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "> item one");

                // Toggle quote off, then wrap in a fenced code block
                let ev = key_event("9", true, false, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "item one");

                let ev = key_event("c", true, true, cx);
                c.on_key(&ev, window, cx);
                assert_eq!(c.editor.doc().text(), "```\nitem one\n```");

                // Undo steps back through block transformations
                c.on_undo(&Undo, window, cx);
                assert_eq!(c.editor.doc().text(), "item one");
            });
        });

        drop(root);
    }

    #[gpui::test]
    fn composer_multiline_list_editing_and_paste_workflows(cx: &mut TestAppContext) {
        use gpui_kit::component::input::Enter;

        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let slot = Rc::new(RefCell::new(None));
        let slot_c = slot.clone();
        let (root, cx) = cx.add_window_view(move |window, cx| {
            let fixtures = Arc::new(crate::fixtures::load());
            let composer = Composer::new("Message".into(), fixtures, window, cx);
            *slot_c.borrow_mut() = Some(composer.clone());
            let view = cx.new(|_| ComposerHarness { composer });
            gpui_kit::base::Root::new(view, window, cx)
        });
        let composer = slot.borrow_mut().take().unwrap();
        cx.run_until_parked();

        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                // Start a bullet list item, Shift+Enter inserts a soft break inside the item
                c.editor.replace_all("- first line");
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: true,
                    },
                    window,
                    cx,
                );
                assert_eq!(c.editor.doc().text(), "- first line\n    ");

                // Type continuation text, then Enter continues with a new bullet
                c.editor.insert_text("continued");
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
                assert_eq!(c.editor.doc().text(), "- first line\n    continued\n- ");

                // Enter on the empty bullet item exits the list cleanly
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
                assert_eq!(c.editor.doc().text(), "- first line\n    continued\n");

                // Multi-line plain text paste returns false (delegates to textarea)
                let multiline = ClipboardItem::new_string("alpha\nbeta\ngamma".into());
                assert!(!c.on_paste(&multiline, window, cx));
                c.editor.replace_all("alpha\nbeta\ngamma");
                assert_eq!(c.editor.doc().text(), "alpha\nbeta\ngamma");

                // URL pasted over selection inside fenced code block must NOT linkify (returns false)
                let url_clip = ClipboardItem::new_string("https://docs.rs".into());
                c.editor.replace_all("```\nlet url = foo;\n```");
                let foo_at = c.editor.doc().text().find("foo").unwrap();
                c.editor.select(foo_at..foo_at + 3);
                assert!(!c.on_paste(&url_clip, window, cx));
                assert_eq!(c.editor.doc().text(), "```\nlet url = foo;\n```");

                // URL pasted over selection inside inline code must NOT linkify
                c.editor.replace_all("see `token` ok");
                let tok_at = c.editor.doc().text().find("token").unwrap();
                c.editor.select(tok_at..tok_at + 5);
                assert!(!c.on_paste(&url_clip, window, cx));
                assert_eq!(c.editor.doc().text(), "see `token` ok");

                // URL pasted at empty caret returns false
                c.editor.replace_all("plain text");
                c.editor.select(5..5);
                assert!(!c.on_paste(&url_clip, window, cx));
            });
        });

        drop(root);
    }

    #[gpui::test]
    fn composer_autocomplete_menu_navigation_pick_escape_and_suppression(cx: &mut TestAppContext) {
        use gpui_kit::component::input::{Enter, Escape, IndentInline, MoveDown, MoveUp};
        use slackless_core::types::{Usergroup, UsergroupId};

        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let slot = Rc::new(RefCell::new(None));
        let slot_c = slot.clone();
        let (root, cx) = cx.add_window_view(move |window, cx| {
            let mut fixtures = crate::fixtures::load();
            fixtures.usergroups.push(Usergroup {
                id: UsergroupId::from("S0BACKEND"),
                handle: "backend-team".into(),
                name: "Backend Team".into(),
                ..Default::default()
            });
            let composer = Composer::new("Message".into(), Arc::new(fixtures), window, cx);
            *slot_c.borrow_mut() = Some(composer.clone());
            let view = cx.new(|_| ComposerHarness { composer });
            gpui_kit::base::Root::new(view, window, cx)
        });
        let composer = slot.borrow_mut().take().unwrap();
        cx.run_until_parked();

        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                // @user with Down/Up wrap navigation and Tab pick preserving surrounding text & caret
                c.editor.replace_all("hello @ world");
                c.editor.select(7..7);
                c.changed(cx);
                let count = c.menu.as_ref().expect("@ menu should open").entries.len();
                assert!(count >= 2);
                assert_eq!(c.menu.as_ref().unwrap().selected, 0);
                c.on_up(&MoveUp, window, cx);
                assert_eq!(c.menu.as_ref().unwrap().selected, count - 1);
                c.on_down(&MoveDown, window, cx);
                assert_eq!(c.menu.as_ref().unwrap().selected, 0);
                c.on_tab(&IndentInline, window, cx);
                assert!(c.menu.is_none());
                assert!(c.editor.doc().text().starts_with("hello @"));
                assert!(c.editor.doc().text().ends_with(" world"));
                assert_eq!(c.editor.doc().atoms().len(), 1);
                let caret = c.editor.selection().start;
                assert_eq!(&c.editor.doc().text()[caret..], "world");

                // @usergroup trigger & Enter pick
                c.editor.replace_all("ping @back");
                c.changed(cx);
                assert!(c.menu.is_some());
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
                assert!(c.editor.doc().to_mrkdwn().contains("<!subteam^S0BACKEND>"));

                // @here broadcast trigger & Tab pick
                c.editor.replace_all("attn @her");
                c.changed(cx);
                assert!(c.menu.is_some());
                c.on_tab(&IndentInline, window, cx);
                assert_eq!(c.editor.doc().to_mrkdwn().trim(), "attn <!here>");

                // #channel trigger & Enter pick
                c.editor.replace_all("see #gen");
                c.changed(cx);
                assert!(c.menu.is_some());
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
                assert!(c.editor.doc().to_mrkdwn().contains("<#C0GENERAL>"));

                // :emoji: trigger & Tab pick
                c.editor.replace_all("nice :rocke");
                c.changed(cx);
                assert!(c.menu.is_some());
                c.on_tab(&IndentInline, window, cx);
                assert!(c.editor.doc().to_mrkdwn().contains(":rocket:"));

                // Esc sticky dismissal stays closed on same trigger, reopens on new trigger
                c.editor.replace_all("hi @");
                c.changed(cx);
                assert!(c.menu.is_some());
                c.on_escape(&Escape, window, cx);
                assert!(c.menu.is_none());
                c.editor.insert_text("a");
                c.changed(cx);
                assert!(c.menu.is_none(), "dismissed trigger stays closed");
                c.editor.insert_text(" #gen");
                c.changed(cx);
                assert!(c.menu.is_some(), "new trigger reopens menu");

                // Non-triggering inside inline code, fenced code blocks, and link URLs
                c.editor.replace_all("`@al`");
                c.editor.select(4..4);
                c.changed(cx);
                assert!(c.menu.is_none(), "no menu inside inline code");

                c.editor.replace_all("```\n#gen\n```");
                c.editor.select(8..8);
                c.changed(cx);
                assert!(c.menu.is_none(), "no menu inside fenced code block");

                c.editor.replace_all("[docs](https://example.com/@al)");
                c.editor.select(30..30);
                c.changed(cx);
                assert!(c.menu.is_none(), "no menu inside link URL");
            });
        });

        drop(root);
    }

    #[gpui::test]
    fn composer_edit_mode_blank_rejection_attachments_and_draft_roundtrip(cx: &mut TestAppContext) {
        use gpui_kit::component::input::{Enter, Escape};

        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });

        type FileSubmitLog = Rc<RefCell<Vec<(Option<String>, Vec<String>)>>>;
        let submits: Rc<RefCell<Vec<MessageContent>>> = Rc::new(RefCell::new(Vec::new()));
        let file_submits: FileSubmitLog = Rc::new(RefCell::new(Vec::new()));
        let cancels: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));

        struct Host {
            composer: Entity<Composer>,
            edit_composer: Entity<Composer>,
            _subs: Vec<Subscription>,
        }
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                v_flex()
                    .size_full()
                    .child(self.composer.clone())
                    .child(self.edit_composer.clone())
            }
        }

        let comp_slot = Rc::new(RefCell::new(None));
        let edit_slot = Rc::new(RefCell::new(None));
        let (comp_s, edit_s) = (comp_slot.clone(), edit_slot.clone());
        let (sub_c, file_c, cancel_c) = (submits.clone(), file_submits.clone(), cancels.clone());

        let (root, cx) = cx.add_window_view(move |window, cx| {
            let fixtures = Arc::new(crate::fixtures::load());
            let composer = Composer::new("Message".into(), fixtures.clone(), window, cx);
            let edit_composer = Composer::new_edit(fixtures, window, cx);
            *comp_s.borrow_mut() = Some(composer.clone());
            *edit_s.borrow_mut() = Some(edit_composer.clone());
            let view = cx.new(|cx| {
                let sub_c2 = sub_c.clone();
                let cancel_c2 = cancel_c.clone();
                let s1 = cx.subscribe(&composer, move |_, _, ev: &ComposerEvent, _| match ev {
                    ComposerEvent::Submit(c) => sub_c.borrow_mut().push(c.clone()),
                    ComposerEvent::SubmitWithFiles { content, files } => {
                        file_c.borrow_mut().push((
                            content.as_ref().map(|c| c.text.clone()),
                            files.iter().map(|f| f.filename.clone()).collect(),
                        ));
                    }
                    _ => {}
                });
                let s2 = cx.subscribe(
                    &edit_composer,
                    move |_, _, ev: &ComposerEvent, _| match ev {
                        ComposerEvent::Submit(c) => sub_c2.borrow_mut().push(c.clone()),
                        ComposerEvent::Cancel => *cancel_c2.borrow_mut() += 1,
                        _ => {}
                    },
                );
                Host {
                    composer,
                    edit_composer,
                    _subs: vec![s1, s2],
                }
            });
            gpui_kit::base::Root::new(view, window, cx)
        });
        let composer = comp_slot.borrow_mut().take().unwrap();
        let edit_composer = edit_slot.borrow_mut().take().unwrap();
        cx.run_until_parked();

        // 1. Empty & whitespace-only send rejection
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                c.editor.replace_all("");
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
                c.editor.replace_all("   \n\t  ");
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
            });
        });
        assert!(submits.borrow().is_empty());

        // 2. Attachment chips add / remove / submit with files
        cx.update(|window, cx| {
            composer.update(cx, |c, cx| {
                let img1 = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3, 4]);
                let img2 = Image::from_bytes(ImageFormat::Jpeg, vec![5, 6, 7]);
                c.attach_clipboard_image(img1, cx);
                c.attach_clipboard_image(img2, cx);
                assert_eq!(c.attachments.len(), 2);
                assert_eq!(c.attachments[0].filename, "image-1.png");
                assert_eq!(c.attachments[1].filename, "image-2.jpg");

                c.remove_attachment(0, cx);
                assert_eq!(c.attachments.len(), 1);
                assert_eq!(c.attachments[0].filename, "image-2.jpg");

                c.editor.replace_all("caption");
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
                assert!(c.attachments.is_empty());
            });
        });
        assert_eq!(
            file_submits.borrow().as_slice(),
            [(Some("caption".to_owned()), vec!["image-2.jpg".to_owned()])]
        );

        // 3. Draft save/restore round-trip preserving rich formatting & atoms
        cx.update(|_, cx| {
            composer.update(cx, |c, cx| {
                let draft = "Hi <@U0ALICE> in <#C0GENERAL> <!here> :rocket:\n\
                             *bold* _italic_ <u>under</u> ~strike~ `code` <https://example.com|docs>\n\
                             - bullet one\n\
                             > quoted note";
                c.load_draft(draft, cx);
                let saved = c.text();
                assert!(saved.contains("<@U0ALICE>"));
                assert!(saved.contains("<#C0GENERAL>"));
                assert!(saved.contains("<!here>"));
                assert!(saved.contains(":rocket:"));
                assert!(saved.contains("*bold*"));
                assert!(saved.contains("_italic_"));
                assert!(saved.contains("<u>under</u>"));
                assert!(saved.contains("~strike~"));
                assert!(saved.contains("`code`"));
                assert!(saved.contains("<https://example.com|docs>"));
                assert!(saved.contains("bullet one"));
                assert!(saved.contains("&gt; quoted note"));

                // Reloading saved draft yields the exact same serialized form & atom count
                let atom_count = c.editor.doc().atoms().len();
                assert_eq!(atom_count, 4);
                c.load_draft(&saved, cx);
                assert_eq!(c.text(), saved);
                assert_eq!(c.editor.doc().atoms().len(), atom_count);
            });
        });

        // 4. Edit-message mode: load_message, Enter submits edit, Esc cancels
        cx.update(|window, cx| {
            edit_composer.update(cx, |c, cx| {
                let msg = Message {
                    text: "Original *bold* message".into(),
                    ..Default::default()
                };
                c.load_message(&msg, window, cx);
                assert_eq!(c.text(), "Original *bold* message");
                c.editor.insert_text(" edited");
                c.on_enter(
                    &Enter {
                        secondary: false,
                        shift: false,
                    },
                    window,
                    cx,
                );
                // Re-load and cancel with Esc
                c.load_message(&msg, window, cx);
                c.on_escape(&Escape, window, cx);
            });
        });
        assert_eq!(submits.borrow().len(), 1);
        assert_eq!(submits.borrow()[0].text, "Original *bold* message edited");
        assert_eq!(*cancels.borrow(), 1);

        drop(root);
    }
}
