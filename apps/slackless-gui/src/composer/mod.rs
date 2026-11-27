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

mod attachments;
mod clips;
mod keys;
mod menu;
mod menu_view;
mod snippet;
mod submit;
mod sync;
mod tokens;
mod toolbar;
mod view;

#[cfg(test)]
mod tests;

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{InputEditsEvent, InputState, TextareaState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, Image,
    ParentElement as _, Pixels, Point, RenderImage, SharedString, Styled as _, Subscription, Task,
    Window, div, px,
};
use slackless_core::api::{GifItem, MessageContent};
use slackless_core::huddle::{AudioClipRecorder, VideoClipRecorder};
use slackless_core::types::{ChannelId, Message};

use crate::drafts::SendLater;
use crate::editor::{Atom, Editor, from_mrkdwn, from_rich_text};
use crate::emoji::EmojiChoice;
use crate::fixtures::Fixtures;
use crate::slash::Invocation;
use menu::Menu;
use tokens::Palette;

pub use submit::content_of;

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
#[derive(Clone, PartialEq, Debug)]
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
    pub(super) fn line(&self) -> (String, Vec<Range<usize>>) {
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

impl AlsoSendTarget {
    /// Derive the "Also send" target label for `channel` from `fixtures`.
    pub fn for_channel(fixtures: &Fixtures, channel: &ChannelId) -> Option<Self> {
        use slackless_core::types::ConversationKind;
        fixtures
            .conversation(channel)
            .map(|conversation| match conversation.kind() {
                ConversationKind::Im | ConversationKind::Mpim => Self::DirectMessage,
                kind => Self::Channel {
                    name: conversation.name.clone().unwrap_or_default().into(),
                    private: kind == ConversationKind::PrivateChannel,
                },
            })
    }
}

/// Format a `/shrug` invocation body with `SHRUG`.
pub(crate) fn format_shrug(args: &str) -> String {
    if args.is_empty() {
        crate::slash::SHRUG.to_owned()
    } else {
        format!("{args} {}", crate::slash::SHRUG)
    }
}

/// Outgoing message privacy/safety preferences cached as a GPUI global so both the main window
/// and popped-out conversation/thread/huddle windows enforce the same send pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutgoingGuardPrefs {
    pub strip_tracking_params: bool,
    pub warn_before_sending_secrets: bool,
}

impl Default for OutgoingGuardPrefs {
    fn default() -> Self {
        Self {
            strip_tracking_params: true,
            warn_before_sending_secrets: true,
        }
    }
}

impl gpui_kit::gpui::Global for OutgoingGuardPrefs {}

pub(crate) fn outgoing_guard_prefs(cx: &App) -> OutgoingGuardPrefs {
    cx.try_global::<OutgoingGuardPrefs>()
        .copied()
        .unwrap_or_default()
}

pub(crate) fn set_outgoing_guard_prefs(
    strip_tracking_params: bool,
    warn_before_sending_secrets: bool,
    cx: &mut App,
) {
    cx.set_global(OutgoingGuardPrefs {
        strip_tracking_params,
        warn_before_sending_secrets,
    });
}

/// Convert common typed emoticons outside inline/fenced code spans into Slack `:shortcode:`
/// emoji when Preferences > Appearance > Convert emoticons is enabled (`F-SHELL-091`).
pub(crate) fn convert_emoticons(text: &str) -> String {
    const MAP: [(&str, &str); 12] = [
        (":-)", ":slightly_smiling_face:"),
        (":)", ":slightly_smiling_face:"),
        (":-D", ":smiley:"),
        (":D", ":smiley:"),
        (":-(", ":disappointed:"),
        (":(", ":disappointed:"),
        (";-)", ":wink:"),
        (";)", ":wink:"),
        (":-P", ":stuck_out_tongue:"),
        (":P", ":stuck_out_tongue:"),
        ("<3", ":heart:"),
        (":/", ":confused:"),
    ];
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('`') {
            in_code = !in_code;
            out.push('`');
            rest = after;
            continue;
        }
        if !in_code {
            let at_boundary = out
                .chars()
                .last()
                .is_none_or(|c| c.is_whitespace() || c == '(');
            if at_boundary
                && let Some((pat, rep)) = MAP.iter().find(|(pat, _)| {
                    rest.strip_prefix(*pat).is_some_and(|after| {
                        after
                            .chars()
                            .next()
                            .is_none_or(|c| c.is_whitespace() || matches!(c, '.' | ',' | '!' | '?'))
                    })
                })
            {
                out.push_str(rep);
                rest = &rest[pat.len()..];
                continue;
            }
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

/// Strip tracking parameters (`utm_*`, `fbclid`, `gclid`, etc.) from outgoing text and
/// rich-text link elements.
pub(crate) fn strip_tracking_params_in_content(content: &mut MessageContent) {
    if !content.text.is_empty() {
        let cleaned = slackless_core::strip_tracking_params_in_text(&content.text);
        if cleaned != content.text {
            content.text = cleaned;
        }
    }
    for block in &mut content.blocks {
        if let slackless_core::types::Block::RichText(rich) = block {
            for container in &mut rich.elements {
                let elements = match container {
                    slackless_core::types::RichTextContainer::Section(s) => &mut s.elements,
                    slackless_core::types::RichTextContainer::Quote(q) => &mut q.elements,
                    slackless_core::types::RichTextContainer::Preformatted(p) => &mut p.elements,
                    slackless_core::types::RichTextContainer::List(list) => {
                        for item in &mut list.elements {
                            if let slackless_core::types::RichTextContainer::Section(s) = item {
                                for el in &mut s.elements {
                                    if let slackless_core::types::RichTextElement::Link(link) = el {
                                        link.url = slackless_core::strip_tracking_params(&link.url);
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    slackless_core::types::RichTextContainer::Header(_)
                    | slackless_core::types::RichTextContainer::Divider(_)
                    | slackless_core::types::RichTextContainer::Unknown { .. } => continue,
                };
                for el in elements {
                    if let slackless_core::types::RichTextElement::Link(link) = el {
                        link.url = slackless_core::strip_tracking_params(&link.url);
                    }
                }
            }
        }
    }
}

/// Apply shared outgoing message transforms (emoticon conversion + tracking-param stripping)
/// respecting active display & privacy preferences.
pub(crate) fn apply_outgoing_transforms(
    content: &mut MessageContent,
    fixtures: &Fixtures,
    cx: &App,
) {
    if crate::preferences::display_prefs(cx).convert_emoticons {
        let converted = convert_emoticons(&content.text);
        if converted != content.text {
            let attachments = std::mem::take(&mut content.attachments);
            *content = content_of(&converted, fixtures);
            content.attachments = attachments;
        }
    }
    if outgoing_guard_prefs(cx).strip_tracking_params {
        strip_tracking_params_in_content(content);
    }
}

/// Open a confirmation dialog when secret-like tokens are detected in an outgoing message.
pub(crate) fn confirm_secret_send_dialog(
    matches: Vec<slackless_core::SecretMatch>,
    window: &mut Window,
    cx: &mut App,
    on_send: impl Fn(&mut Window, &mut App) + 'static,
) {
    use gpui_kit::gpui::{FontWeight, IntoElement as _};

    let matches = Rc::new(matches);
    crate::ui::ConfirmDialog::new(
        "Possible secret detected",
        "This message appears to contain sensitive credentials. Sending it to Slack cannot be undone.",
        "Send anyway",
    )
    .danger()
    .width(px(460.))
    .extra(move |cx| {
        let theme = cx.theme();
        v_flex()
            .gap_1p5()
            .p_2p5()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary.opacity(0.45))
            .children(matches.iter().map(|m| {
                h_flex()
                    .justify_between()
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.danger)
                            .child(m.kind),
                    )
                    .child(div().text_color(theme.muted_foreground).child(m.preview.clone()))
            }))
            .into_any_element()
    })
    .open(window, cx, on_send);
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

/// App/built-in target chosen from the `/` autocomplete menu when disambiguating duplicate
/// slash command names (`F-COLLAB-686`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SelectedSlashTarget {
    pub name: String,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub is_builtin: bool,
}

pub struct Composer {
    /// The textarea the draft is typed in.
    input: Entity<TextareaState>,
    /// What the draft is: its formatting, mentions, blocks and undo history.
    editor: Editor,
    fixtures: Arc<Fixtures>,
    /// The completion menu open at the caret.
    menu: Option<Menu>,
    /// Explicit slash-command target picked from autocomplete (`F-COLLAB-686`).
    selected_slash_target: Option<SelectedSlashTarget>,
    /// Optional broadcast permission/size badge shown on `@channel`/`@here`/`@everyone` rows
    /// (`F-COLLAB-1248`, `F-COLLAB-1249`).
    broadcast_hint: Option<SharedString>,
    /// Optional Slack Connect external-sharing warning shown when files are staged in an
    /// externally shared channel (`F-COLLAB-879`).
    external_file_notice: Option<SharedString>,
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

/// Display string for an [`EmojiChoice`] inserted into the WYSIWYG editor.
fn emoji_display_text(choice: &EmojiChoice) -> String {
    choice
        .glyph
        .clone()
        .unwrap_or_else(|| format!(":{}:", choice.name))
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
                selected_slash_target: None,
                broadcast_hint: None,
                external_file_notice: None,
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

    /// Show `bar` inside the frame above the formatting bar, or remove it.
    pub fn set_context_bar(&mut self, bar: Option<ContextBar>, cx: &mut Context<Self>) {
        self.context_bar = bar;
        cx.notify();
    }

    /// Update the broadcast mention hint shown on `@channel`/`@here`/`@everyone` autocomplete
    /// rows for the open conversation (`F-COLLAB-1248`, `F-COLLAB-1249`).
    pub fn set_broadcast_hint(&mut self, hint: Option<SharedString>, cx: &mut Context<Self>) {
        if self.broadcast_hint != hint {
            self.broadcast_hint = hint;
            cx.notify();
        }
    }

    /// Update the Slack Connect external file-sharing notice shown when files are staged in
    /// an externally shared channel (`F-COLLAB-879`).
    pub fn set_external_file_notice(
        &mut self,
        notice: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if self.external_file_notice != notice {
            self.external_file_notice = notice;
            cx.notify();
        }
    }

    /// Prompt with `placeholder` while empty ("Message #general"); applied on the next frame.
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

    /// The files staged for the next send, in strip order.
    #[cfg(test)]
    pub fn attachments(&self) -> &[PendingAttachment] {
        &self.attachments
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
        let shown = emoji_display_text(choice);
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
}

pub(crate) fn format_bytes(bytes: usize) -> String {
    slackless_core::types::time::format_bytes(bytes as u64)
}
