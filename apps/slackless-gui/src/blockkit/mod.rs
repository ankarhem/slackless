//! Block Kit rendering: the app-authored blocks a client must display, and legacy
//! attachments.
//!
//! `slackless_types` models only `rich_text` (rendered by `messages.rs`); every other block
//! arrives as `Block::Unknown { r#type, raw }` with its JSON preserved, which this module
//! renders: `section` (text, `fields`, `accessory`), `divider`, `image`, `context`, `header`,
//! `actions`, and `input`. Interactive elements are real gpui-kit widgets: `Button`s (primary /
//! danger / link styles), dropdown menus for `static_select` / `users_select` /
//! `conversations_select` / `overflow` / `datepicker` / `timepicker`, `Checkbox` and `Radio`
//! groups, and `Textarea`s for text inputs. An interaction updates the local
//! [`BlockKitState`] (selection, checks, drafts); a button press in live mode goes to the app
//! that posted the message (`blocks.actions`, see [`BlockDispatch`]), and fixture mode logs it
//! to stderr with a toast.
//!
//! Legacy attachments render Slack's way: a color bar (hex or `good`/`warning`/`danger`)
//! beside pretext, author, linked title, mrkdwn text, short/long fields, nested blocks, an
//! image and the footer line.
//!
//! Design tokens: colors via `cx.theme()`; text via the shared mrkdwn renderer in
//! `messages.rs`; images through [`Fixtures::block_images`] (live mode: cookie-authenticated
//! downloads), a framed placeholder otherwise.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, Size, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Div, Entity, FontWeight, Hsla, ImageSource,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, img, px,
};
use slackless_core::api::ActionContainer;
use slackless_core::domain::mrkdwn::unescape_mrkdwn;
use slackless_core::types::{
    Attachment, Block, BotId, ChannelId, ConversationKind, Message, TeamId, Ts, UserId,
};

use crate::actions;
use crate::avatars;
use crate::emoji;
use crate::fixtures::Fixtures;
use crate::image_viewer::ImageOrigin;
use crate::messages::{self, LinkHandler, LinkTarget, THEME_ORDER_STRIDE};

/// Re-export workspace `serde_json` for Block Kit modules and callers that import it from here.
pub(crate) use ::serde_json;
pub(crate) type Json = serde_json::Value;

/// Local state of interactive Block Kit controls, keyed by [`control_key`].
#[derive(Default)]
pub struct BlockKitState {
    /// Chosen option value of selects, radio groups and date/time pickers.
    choices: HashMap<String, String>,
    /// Checked option values of checkbox groups (absent: the block's `initial_options`).
    checked: HashMap<String, HashSet<String>>,
    /// Drafts of text inputs; created on first render because a textarea needs the window.
    inputs: HashMap<String, Entity<TextareaState>>,
    /// Attachments whose long text the user expanded ("Show more"), by message and index.
    expanded: HashSet<String>,
    /// Image blocks the user collapsed with their title's caret, by message and block.
    collapsed: HashSet<String>,
    /// Link unfurl cards the user dismissed locally, by message ts and attachment index.
    dismissed_unfurls: HashSet<String>,
    /// Datepicker keys whose monthly calendar grid is currently expanded (`F-COLLAB-745`).
    open_calendars: HashSet<String>,
    /// Currently browsed `(year, month)` per datepicker key (1-based month).
    calendar_months: HashMap<String, (i32, u32)>,
    /// Live option suggestions fetched via `blocks.suggestions` for `external_select` (`F-COLLAB-786`).
    suggestions: HashMap<String, Vec<(String, String)>>,
    /// Query inputs for `external_select` / `multi_external_select` elements.
    suggestion_queries: HashMap<String, Entity<TextareaState>>,
    /// Keys currently waiting on an in-flight `blocks.suggestions` request.
    suggestions_loading: HashSet<String>,
    /// Monotonic per-control sequence number for `blocks.suggestions` requests so stale
    /// out-of-order responses never overwrite newer queries.
    suggestion_seq: HashMap<String, u64>,
    /// Most recent in-flight query string per `external_select` control key for deduping.
    suggestion_last_query: HashMap<String, String>,
    /// Inline validation errors keyed by `block_id` (`views.submit` `response_action: "errors"`
    /// and local required/typed field checks, `F-COLLAB-612`).
    block_errors: HashMap<String, String>,
    /// Hydrated third-party `calls.info` payloads keyed by `call_id` (`F-COLLAB-087`).
    hydrated_calls: HashMap<String, slackless_core::api::CallInfo>,
    /// Locally chosen file paths for `file_input` elements in modals (`F-COLLAB-751`).
    files: HashMap<String, Vec<String>>,
}

impl BlockKitState {
    /// Start a sequenced `blocks.suggestions` request for `base_key` with `query`.
    /// Returns `None` if an identical query is already in flight for `base_key`, avoiding
    /// duplicate rapid clicks while still allowing a newer query to supersede a slow request.
    pub fn begin_suggestion_request(&mut self, base_key: &str, query: &str) -> Option<String> {
        if self.suggestions_loading.contains(base_key)
            && self
                .suggestion_last_query
                .get(base_key)
                .is_some_and(|prev| prev == query)
        {
            return None;
        }
        let next = self
            .suggestion_seq
            .get(base_key)
            .copied()
            .unwrap_or(0)
            .wrapping_add(1);
        self.suggestion_seq.insert(base_key.to_owned(), next);
        self.suggestion_last_query
            .insert(base_key.to_owned(), query.to_owned());
        self.suggestions_loading.insert(base_key.to_owned());
        Some(format!("{base_key}#seq={next}"))
    }

    /// Store dynamically loaded `blocks.suggestions` options for `key`.
    /// When `key` carries a `#seq=N` suffix from [`Self::begin_suggestion_request`], stale
    /// out-of-order responses whose sequence number does not match the latest request are dropped.
    pub fn set_suggestions(&mut self, key: String, options: Vec<(String, String)>) {
        let (base_key, seq) = match key.rsplit_once("#seq=") {
            Some((base, raw_seq)) => match raw_seq.parse::<u64>() {
                Ok(n) => (base.to_owned(), Some(n)),
                Err(_) => (key, None),
            },
            None => (key, None),
        };
        if let Some(seq) = seq
            && self.suggestion_seq.get(&base_key).copied() != Some(seq)
        {
            return;
        }
        self.suggestions_loading.remove(&base_key);
        self.suggestion_last_query.remove(&base_key);
        self.suggestions.insert(base_key, options);
    }

    /// Replace all inline `block_id -> error` validation messages (e.g. from `views.submit`).
    pub fn set_block_errors(&mut self, errors: HashMap<String, String>) {
        self.block_errors = errors;
    }

    /// Clear the inline validation error for `block_id` once the user edits that input.
    pub fn clear_block_error(&mut self, block_id: &str) {
        self.block_errors.remove(block_id);
    }

    /// Current inline validation error for `block_id`, if any.
    pub fn block_error(&self, block_id: &str) -> Option<&str> {
        self.block_errors.get(block_id).map(String::as_str)
    }

    /// Cache a hydrated `calls.info` payload for live `call` block updates (`F-COLLAB-087`).
    pub fn set_call_info(&mut self, call_id: String, info: slackless_core::api::CallInfo) {
        self.hydrated_calls.insert(call_id, info);
    }

    /// Look up hydrated `calls.info` metadata for `call_id`.
    pub fn call_info(&self, call_id: &str) -> Option<&slackless_core::api::CallInfo> {
        self.hydrated_calls.get(call_id)
    }
}

/// Request payload emitted when an `external_select` / `multi_external_select` queries
/// `blocks.suggestions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestRequest {
    pub key: String,
    pub action_id: String,
    pub block_id: String,
    pub value: String,
    pub attachment: Option<i64>,
}

/// Callback invoked when an `external_select` needs live options from `blocks.suggestions`.
pub type SuggestHandler = std::rc::Rc<dyn Fn(SuggestRequest, &mut App)>;

/// Request to inspect a workflow trigger's details (`workflows.triggers.info`, `F-COLLAB-432`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDetailsRequest {
    pub trigger_id: Option<String>,
    pub url: Option<String>,
    pub title: String,
}

/// Callback invoked when a user asks to view workflow details from a workflow button or chip.
pub type WorkflowDetailsHandler =
    std::rc::Rc<dyn Fn(WorkflowDetailsRequest, &mut Window, &mut App)>;

/// Callback invoked when a user asks to refresh a `call` block via `calls.info`.
pub type CallRefreshHandler = std::rc::Rc<dyn Fn(String, &mut App)>;

/// What one message's blocks render with.
pub struct KitCx<'a> {
    pub fixtures: &'a Fixtures,
    /// Interactive state; `None` renders controls from the payload's initial values only.
    pub kit: Option<&'a Entity<BlockKitState>>,
    pub ts: &'a Ts,
    /// Routing for this message's text links; with a handler the text is also selectable
    /// (see [`crate::messages`]).
    pub links: Option<LinkHandler>,
    /// The click handler opening the image viewer (the transcript provides it; static
    /// cards render their images without it).
    pub image_open: Option<crate::image_viewer::ImageClick>,
    /// Where this message's button presses go (the transcript's and thread panel's message
    /// actions); without it a press is only logged.
    pub on_action: Option<actions::ActionHandler>,
    /// Optional callback for fetching `external_select` options (`blocks.suggestions`).
    pub on_suggest: Option<SuggestHandler>,
    /// Optional callback for opening the Workflow Details dialog (`workflows.triggers.info`).
    pub on_workflow_details: Option<WorkflowDetailsHandler>,
    /// Optional callback for hydrating a `call` block via `calls.info`.
    pub on_call_refresh: Option<CallRefreshHandler>,
    /// The legacy attachment these blocks sit in, by its `id` (a button's container names it).
    attachment: Option<i64>,
    /// The next selection-order slice handed to a text theme (see [`messages::mrkdwn`]).
    order: std::cell::Cell<u64>,
    /// Keys handed to inline text renderers, so their element ids stay unique across the
    /// blocks and attachments of one message (GPUI keys a11y nodes by id alone).
    keys: std::cell::Cell<usize>,
}

impl<'a> KitCx<'a> {
    pub fn new(fixtures: &'a Fixtures, kit: Option<&'a Entity<BlockKitState>>, ts: &'a Ts) -> Self {
        Self {
            fixtures,
            kit,
            ts,
            links: None,
            image_open: None,
            on_action: None,
            on_suggest: None,
            on_workflow_details: None,
            on_call_refresh: None,
            attachment: None,
            order: std::cell::Cell::new(0),
            keys: std::cell::Cell::new(0),
        }
    }

    /// Routes this message's button presses through `on_action`, as
    /// [`MessageAction::BlockAction`](actions::MessageAction::BlockAction).
    pub fn dispatching_actions(mut self, on_action: actions::ActionHandler) -> Self {
        self.on_action = Some(on_action);
        self
    }

    /// Routes `external_select` / `multi_external_select` option queries through `on_suggest`.
    pub fn with_suggestions(mut self, on_suggest: SuggestHandler) -> Self {
        self.on_suggest = Some(on_suggest);
        self
    }

    /// Routes workflow details requests through `on_workflow_details`.
    pub fn with_workflow_details(mut self, on_workflow_details: WorkflowDetailsHandler) -> Self {
        self.on_workflow_details = Some(on_workflow_details);
        self
    }

    /// Routes `call` block live refresh requests through `on_call_refresh`.
    pub fn with_call_refresh(mut self, on_call_refresh: CallRefreshHandler) -> Self {
        self.on_call_refresh = Some(on_call_refresh);
        self
    }

    /// Routes this message's text links through `links` (also makes its text selectable).
    pub fn with_links(mut self, links: LinkHandler) -> Self {
        self.links = Some(links);
        self
    }

    /// Routes this message's image clicks to the image viewer (the transcript provides
    /// the handler; see [`crate::image_viewer`]).
    pub fn opening_images(mut self, on_open: crate::image_viewer::ImageClick) -> Self {
        self.image_open = Some(on_open);
        self
    }

    /// A [`KitCx`] for a nested render (an attachment's own blocks): fresh keys, the parent's
    /// link routing and the selection order continuing after the parent's own text.
    pub fn nested<'b>(
        &self,
        fixtures: &'b Fixtures,
        kit: Option<&'b Entity<BlockKitState>>,
        ts: &'b Ts,
    ) -> KitCx<'b> {
        KitCx {
            fixtures,
            kit,
            ts,
            links: self.links.clone(),
            image_open: self.image_open.clone(),
            on_action: self.on_action.clone(),
            on_suggest: self.on_suggest.clone(),
            on_workflow_details: self.on_workflow_details.clone(),
            on_call_refresh: self.on_call_refresh.clone(),
            attachment: self.attachment,
            order: std::cell::Cell::new(self.order.get()),
            keys: std::cell::Cell::new(0),
        }
    }

    /// A fresh text-element key plus the selection-order base for one text theme of this
    /// message: each theme owns [`THEME_ORDER_STRIDE`] paragraph slots of the message's
    /// document order, so paragraphs of different blocks never interleave.
    pub fn text_theme(&self) -> (String, u64) {
        let key = self.text_key();
        let base = messages::message_order(self.ts) + self.order.get();
        self.order.set(self.order.get() + THEME_ORDER_STRIDE);
        (key, base)
    }

    /// A fresh text-element key for this message.
    pub fn text_key(&self) -> String {
        let n = self.keys.get();
        self.keys.set(n + 1);
        format!("{}-t{n}", self.ts.as_str())
    }
}

/// Stable key of a control: message ts, block id (or index), action id (or index).
fn control_key(
    ts: &Ts,
    block: &Json,
    block_ix: usize,
    element: &Json,
    element_ix: usize,
) -> String {
    let block_id = block["block_id"]
        .as_str()
        .map_or_else(|| block_ix.to_string(), str::to_owned);
    let action_id = element["action_id"]
        .as_str()
        .map_or_else(|| element_ix.to_string(), str::to_owned);
    format!("{}/{block_id}/{action_id}", ts.as_str())
}

const TEXT_INPUTS: [&str; 5] = [
    "plain_text_input",
    "email_text_input",
    "url_text_input",
    "number_input",
    "rich_text_input",
];

fn extract_rich_text_plain(value: &Json) -> String {
    if let Some(s) = value.as_str() {
        return s.to_owned();
    }
    let mut out = String::new();
    fn walk(node: &Json, out: &mut String) {
        if let Some(t) = node["text"].as_str() {
            out.push_str(t);
        }
        if let Some(arr) = node["elements"].as_array() {
            for child in arr {
                walk(child, out);
            }
        }
    }
    walk(value, &mut out);
    out
}

/// Give each text input and `external_select` query input of `message` its textarea state
/// (needs the window, so the transcript / modal calls this before painting).
pub fn prepare_inputs(
    kit: &Entity<BlockKitState>,
    message: &Message,
    window: &mut Window,
    cx: &mut App,
) {
    let ensure_suggest_query = |kit: &Entity<BlockKitState>,
                                key: String,
                                element: &Json,
                                window: &mut Window,
                                cx: &mut App| {
        if kit.read(cx).suggestion_queries.contains_key(&key) {
            return;
        }
        let min_len = element["min_query_length"].as_u64().unwrap_or(0);
        let placeholder = if min_len > 0 {
            format!("Type {min_len}+ chars to search\u{2026}")
        } else {
            text_of(&element["placeholder"])
                .map(|p| format!("Search {p}\u{2026}"))
                .unwrap_or_else(|| "Search options\u{2026}".to_owned())
        };
        kit.update(cx, |state, cx| {
            let query = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(placeholder)
                    .auto_grow(1, 1)
            });
            state.suggestion_queries.insert(key, query);
        });
    };

    for (ix, block) in message.blocks.iter().enumerate() {
        let Block::Unknown { r#type, raw } = block else {
            continue;
        };
        if r#type == "section"
            && let Some(acc) = raw.get("accessory").filter(|v| v.is_object())
            && matches!(
                acc["type"].as_str(),
                Some("external_select" | "multi_external_select")
            )
        {
            let key = control_key(&message.ts, raw, ix, acc, 0);
            ensure_suggest_query(kit, key, acc, window, cx);
            continue;
        }
        if r#type == "actions"
            && let Some(elements) = raw["elements"].as_array()
        {
            for (n, el) in elements.iter().enumerate() {
                if matches!(
                    el["type"].as_str(),
                    Some("external_select" | "multi_external_select")
                ) {
                    let key = control_key(&message.ts, raw, ix, el, n);
                    ensure_suggest_query(kit, key, el, window, cx);
                }
            }
            continue;
        }
        let element = &raw["element"];
        let kind = element["type"].as_str().unwrap_or_default();
        if r#type != "input" {
            continue;
        }
        if matches!(kind, "external_select" | "multi_external_select") {
            let key = control_key(&message.ts, raw, ix, element, 0);
            ensure_suggest_query(kit, key, element, window, cx);
            continue;
        }
        if !TEXT_INPUTS.contains(&kind) {
            continue;
        }
        let key = control_key(&message.ts, raw, ix, element, 0);
        if kit.read(cx).inputs.contains_key(&key) {
            continue;
        }
        let placeholder = text_of(&element["placeholder"])
            .unwrap_or_default()
            .to_owned();
        let initial = if kind == "rich_text_input" {
            extract_rich_text_plain(&element["initial_value"])
        } else {
            element["initial_value"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        };
        let rows = if kind == "rich_text_input" || element["multiline"].as_bool().unwrap_or(false) {
            3
        } else {
            1
        };
        kit.update(cx, |state, cx| {
            let input = cx.new(|cx| {
                let mut input = TextareaState::new(window, cx)
                    .placeholder(placeholder)
                    .auto_grow(rows, 8);
                input.set_value(initial, window, cx);
                input
            });
            state.inputs.insert(key, input);
        });
    }
}

/// `label` as a clickable link routing through the message's link handler (a permalink
/// opens in-app; without a handler the URL opens in the browser).
fn routed_link(
    id: String,
    label: String,
    url: String,
    kcx: &KitCx,
    weight: FontWeight,
    cx: &App,
) -> gpui_kit::gpui::Stateful<Div> {
    let target = LinkTarget::Url(url.into());
    let links = kcx.links.clone();
    div()
        .id(SharedString::from(id))
        .font_weight(weight)
        .text_color(cx.theme().link)
        .cursor_pointer()
        .hover(|el| el.underline())
        .on_click(move |_, window, cx| {
            messages::route_link(links.as_ref(), target.clone(), window, cx)
        })
        .child(label)
}

/// The `text` of a text object.
fn text_of(object: &Json) -> Option<&str> {
    object["text"].as_str()
}

/// Decode Slack HTML entities (`&amp;`, `&lt;`, `&gt;`) and substitute `:shortcode:` emoji
/// unless Preferences -> Messages & media -> Emoji text mode is enabled.
fn plain_label(text: &str, cx: &App) -> String {
    let decoded = unescape_mrkdwn(text);
    if crate::preferences::display_prefs(cx).emoji_text_mode {
        decoded.into_owned()
    } else {
        emoji::replace_shortcodes(&decoded)
    }
}

/// A text object rendered by its type: mrkdwn through the shared renderer (honoring `verbatim`,
/// `F-COLLAB-758`), plain_text with HTML entities decoded and emoji shortcodes substituted.
fn text_object(object: &Json, kcx: &KitCx, muted: bool, cx: &App) -> Option<AnyElement> {
    let text = text_of(object)?;
    if object["type"].as_str() == Some("mrkdwn") {
        let verbatim = object["verbatim"].as_bool().unwrap_or(false);
        return Some(
            messages::mrkdwn_with_verbatim(text, kcx, muted, false, verbatim, cx)
                .into_any_element(),
        );
    }
    Some(div().child(plain_label(text, cx)).into_any_element())
}

/// One top-level (or attachment) block that is not `rich_text`.
pub fn block(kind: &str, raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Option<AnyElement> {
    let theme = cx.theme();
    let element =
        match kind {
            "section" => section(raw, ix, kcx, cx).into_any_element(),
            "divider" => div()
                .w_full()
                .h(px(1.))
                .my_1()
                .bg(theme.border)
                .into_any_element(),
            "header" => div()
                .pt_1()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .child(plain_label(text_of(&raw["text"])?, cx))
                .into_any_element(),
            "image" => image_block(raw, kcx, cx).into_any_element(),
            "table" => table_block(raw, ix, kcx, cx).into_any_element(),
            "data_table" => data_table_block(raw, ix, kcx, cx).into_any_element(),
            // Slack's context row: 20px images, tags, citations, and 13px muted text, 4px apart.
            "context" => h_flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .children(raw["elements"].as_array()?.iter().enumerate().filter_map(
                    |(n, element)| match element["type"].as_str()? {
                        "image" => Some(image_element(element, kcx, 20., cx)),
                        "tag" => Some(tag_element(element, cx).into_any_element()),
                        "citation" => Some(citation_element(element, ix * 19 + n, kcx, cx)),
                        "color" => Some(color_element(element, cx).into_any_element()),
                        "url_source" => Some(url_source_element(element, ix * 19 + n, kcx, cx)),
                        "work_object" => Some(work_object_element(element, ix * 19 + n, kcx, cx)),
                        "workflow" => Some(workflow_link_element(element, ix * 19 + n, kcx, cx)),
                        "file" => Some(file_block(element, cx).into_any_element()),
                        "canvas" | "canvas_unfurl" => {
                            Some(canvas_element(element, ix * 19 + n, kcx, cx).into_any_element())
                        }
                        "list_record" | "slack_list_record" => Some(
                            list_record_element(element, ix * 19 + n, kcx, cx).into_any_element(),
                        ),
                        _ => text_object(element, kcx, true, cx),
                    },
                ))
                .into_any_element(),
            "actions" => h_flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .children(
                    raw["elements"]
                        .as_array()?
                        .iter()
                        .enumerate()
                        .map(|(n, element)| {
                            let key = control_key(kcx.ts, raw, ix, element, n);
                            block_element(element, raw, key, kcx, cx)
                        }),
                )
                .into_any_element(),
            "input" => input_block(raw, ix, kcx, cx).into_any_element(),
            "rich_text" => {
                if let Ok(parsed) = serde_json::from_value::<Block>(raw.clone()) {
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .children(messages::render_block(&parsed, 0, kcx, false, cx))
                        .into_any_element()
                } else {
                    return None;
                }
            }
            "markdown" => {
                let text = raw["text"].as_str()?;
                let converted = standard_markdown_to_mrkdwn(text);
                messages::mrkdwn(&converted, kcx, false, false, cx).into_any_element()
            }
            "video" => video_block(raw, ix, kcx, cx).into_any_element(),
            "call" => call_block(raw, ix, kcx, cx).into_any_element(),
            "alert" | "callout" => alert_block(raw, ix, kcx, cx).into_any_element(),
            "card" | "container" => card_block(raw, ix, kcx, cx).into_any_element(),
            "carousel" => carousel_block(raw, ix, kcx, cx).into_any_element(),
            "context_actions" => context_actions_block(raw, ix, kcx, cx).into_any_element(),
            "task_card" => task_card_block(raw, ix, kcx, cx).into_any_element(),
            "plan" => plan_block(raw, ix, kcx, cx).into_any_element(),
            "file" => file_block(raw, cx).into_any_element(),
            "data_visualization" | "chart" => data_visualization_block(raw, cx).into_any_element(),
            "layout" | "columns" => layout_block(raw, ix, kcx, cx).into_any_element(),
            "canvas" | "canvas_unfurl" => canvas_element(raw, ix, kcx, cx).into_any_element(),
            "list_record" | "slack_list_record" => {
                list_record_element(raw, ix, kcx, cx).into_any_element()
            }
            other => div()
                .text_xs()
                .italic()
                .text_color(theme.muted_foreground)
                .child(format!(
                    "This message contains a {other} block this client cannot show yet."
                ))
                .into_any_element(),
        };
    Some(element)
}

/// Convert common standard Markdown constructs used by Slack's `markdown` block (`**bold**`,
/// `[label](url)`, `# Heading`, `~~strike~~`) into Slack `mrkdwn` while leaving fenced and
/// inline code spans untouched.
fn standard_markdown_to_mrkdwn(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_fence = false;
    for (line_ix, line) in input.split('\n').enumerate() {
        if line_ix > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        let trimmed = line.trim_start();
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
            let heading = trimmed[hashes + 1..].trim();
            out.push('*');
            out.push_str(&convert_inline_markdown(heading));
            out.push('*');
        } else {
            out.push_str(&convert_inline_markdown(line));
        }
    }
    out
}

fn convert_inline_markdown(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_code = false;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            in_code = !in_code;
            out.push('`');
            i += 1;
            continue;
        }
        if in_code {
            let Some(ch) = line[i..].chars().next() else {
                break;
            };
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        // `[label](url)` -> `<url|label>`
        if bytes[i] == b'['
            && let Some(close_bracket) = line[i + 1..].find("](")
        {
            let label_end = i + 1 + close_bracket;
            let url_start = label_end + 2;
            if let Some(close_paren) = line[url_start..].find(')') {
                let label = &line[i + 1..label_end];
                let url = &line[url_start..url_start + close_paren];
                if !url.is_empty() && !url.contains(char::is_whitespace) {
                    out.push('<');
                    out.push_str(url);
                    if !label.is_empty() {
                        out.push('|');
                        out.push_str(label);
                    }
                    out.push('>');
                    i = url_start + close_paren + 1;
                    continue;
                }
            }
        }
        // `**bold**` -> `*bold*`, `~~strike~~` -> `~strike~`
        if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            out.push('*');
            i += 2;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'~' && bytes[i + 1] == b'~' {
            out.push('~');
            i += 2;
            continue;
        }
        let Some(ch) = line[i..].chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

mod attachments;
mod blocks;
mod elements;
#[cfg(test)]
mod tests;

pub use attachments::attachments;
use blocks::*;
pub use elements::BlockDispatch;
pub(crate) use elements::log_button;
use elements::*;

/// Prepare text input controls for raw modal JSON blocks (`views.open`, `F-COLLAB-725`).
pub fn prepare_raw_inputs(
    kit: &Entity<BlockKitState>,
    ts: &Ts,
    raw_blocks: &[Json],
    window: &mut Window,
    cx: &mut App,
) {
    let synthetic = Message {
        ts: ts.clone(),
        blocks: raw_blocks
            .iter()
            .map(|raw| Block::Unknown {
                r#type: raw["type"].as_str().unwrap_or_default().to_owned(),
                raw: raw.clone(),
            })
            .collect(),
        ..Default::default()
    };
    prepare_inputs(kit, &synthetic, window, cx);
}

/// Render raw Block Kit JSON blocks (used by interactive app modals from `views.open`).
pub fn render_raw_blocks(raw_blocks: &[Json], kcx: &KitCx, cx: &App) -> Div {
    let mut column = v_flex().gap_3().w_full().min_w_0();
    for (ix, raw) in raw_blocks.iter().enumerate() {
        let r#type = raw["type"].as_str().unwrap_or_default();
        if let Some(el) = block(r#type, raw, ix, kcx, cx) {
            column = column.child(el);
        }
    }
    column
}

/// Collect Slack `state.values` from modal `input` blocks and validate required / typed inputs,
/// returning either the `state.values` map or per-`block_id` inline validation errors.
pub fn collect_modal_state_errors(
    ts: &Ts,
    raw_blocks: &[Json],
    kit: &Entity<BlockKitState>,
    cx: &App,
) -> Result<Json, HashMap<String, String>> {
    let mut values = serde_json::Map::new();
    let mut errors = HashMap::new();
    let state = kit.read(cx);
    for (ix, raw) in raw_blocks.iter().enumerate() {
        if raw["type"].as_str() != Some("input") {
            continue;
        }
        let element = &raw["element"];
        let kind = element["type"].as_str().unwrap_or_default();
        let block_id = raw["block_id"]
            .as_str()
            .map_or_else(|| ix.to_string(), str::to_owned);
        let action_id = element["action_id"]
            .as_str()
            .map_or_else(|| "0".to_owned(), str::to_owned);
        let optional = raw["optional"].as_bool().unwrap_or(false);
        let label = text_of(&raw["label"]).unwrap_or(&block_id);
        let key = control_key(ts, raw, ix, element, 0);

        let entry = if TEXT_INPUTS.contains(&kind) {
            let val = state
                .inputs
                .get(&key)
                .map(|i| i.read(cx).value().to_string())
                .or_else(|| {
                    if kind == "rich_text_input" {
                        Some(extract_rich_text_plain(&element["initial_value"]))
                    } else {
                        element["initial_value"].as_str().map(str::to_owned)
                    }
                })
                .unwrap_or_default();
            if !optional && val.trim().is_empty() {
                errors.insert(block_id.clone(), format!("{label} is required."));
                continue;
            }
            if let Some(err) = validate_text_input(element, &val) {
                errors.insert(block_id.clone(), format!("{label}: {err}"));
                continue;
            }
            if kind == "rich_text_input" {
                serde_json::json!({
                    "type": kind,
                    "rich_text_value": rich_text_input_value(&val),
                })
            } else {
                serde_json::json!({ "type": kind, "value": val })
            }
        } else if kind == "file_input" {
            let files = state.files.get(&key).cloned().unwrap_or_default();
            if !optional && files.is_empty() {
                errors.insert(block_id.clone(), format!("{label} is required."));
                continue;
            }
            let arr: Vec<Json> = files
                .into_iter()
                .map(|path| serde_json::json!({ "name": path }))
                .collect();
            serde_json::json!({ "type": kind, "files": arr })
        } else if kind == "datepicker" {
            let val = state
                .choices
                .get(&key)
                .cloned()
                .or_else(|| element["initial_date"].as_str().map(str::to_owned));
            if !optional && val.is_none() {
                errors.insert(block_id.clone(), format!("{label} is required."));
                continue;
            }
            serde_json::json!({ "type": kind, "selected_date": val })
        } else if kind == "timepicker" {
            let val = state
                .choices
                .get(&key)
                .cloned()
                .or_else(|| element["initial_time"].as_str().map(str::to_owned));
            if !optional && val.is_none() {
                errors.insert(block_id.clone(), format!("{label} is required."));
                continue;
            }
            serde_json::json!({ "type": kind, "selected_time": val })
        } else if kind == "checkboxes" || kind.starts_with("multi_") {
            let selected: Vec<String> = if let Some(set) = state.checked.get(&key) {
                let mut v: Vec<String> = set.iter().cloned().collect();
                v.sort();
                v
            } else {
                element["initial_options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| o["value"].as_str().map(str::to_owned))
                    .chain(
                        element["initial_users"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_str().map(str::to_owned)),
                    )
                    .chain(
                        element["initial_conversations"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_str().map(str::to_owned)),
                    )
                    .chain(
                        element["initial_channels"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_str().map(str::to_owned)),
                    )
                    .collect()
            };
            if !optional && selected.is_empty() {
                errors.insert(block_id.clone(), format!("{label} is required."));
                continue;
            }
            match kind {
                "multi_users_select" => {
                    serde_json::json!({ "type": kind, "selected_users": selected })
                }
                "multi_conversations_select" => {
                    serde_json::json!({ "type": kind, "selected_conversations": selected })
                }
                "multi_channels_select" => {
                    serde_json::json!({ "type": kind, "selected_channels": selected })
                }
                _ => {
                    let arr: Vec<Json> = selected
                        .into_iter()
                        .map(|v| serde_json::json!({ "value": v }))
                        .collect();
                    serde_json::json!({ "type": kind, "selected_options": arr })
                }
            }
        } else {
            let val = state
                .choices
                .get(&key)
                .cloned()
                .or_else(|| {
                    element["initial_option"]["value"]
                        .as_str()
                        .map(str::to_owned)
                })
                .or_else(|| element["initial_user"].as_str().map(str::to_owned))
                .or_else(|| element["initial_conversation"].as_str().map(str::to_owned))
                .or_else(|| element["initial_channel"].as_str().map(str::to_owned));
            if !optional && val.is_none() {
                errors.insert(block_id.clone(), format!("{label} is required."));
                continue;
            }
            match kind {
                "users_select" => serde_json::json!({ "type": kind, "selected_user": val }),
                "conversations_select" => {
                    serde_json::json!({ "type": kind, "selected_conversation": val })
                }
                "channels_select" => serde_json::json!({ "type": kind, "selected_channel": val }),
                _ => serde_json::json!({
                    "type": kind,
                    "selected_option": val.map(|v| serde_json::json!({ "value": v })),
                }),
            }
        };

        let block_map = values
            .entry(block_id)
            .or_insert_with(|| Json::Object(serde_json::Map::new()));
        if let Some(obj) = block_map.as_object_mut() {
            obj.insert(action_id, entry);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Json::Object(values))
}

/// Collect Slack `state.values` from modal `input` blocks and validate required / typed inputs.
#[cfg(test)]
pub fn collect_modal_state(
    ts: &Ts,
    raw_blocks: &[Json],
    kit: &Entity<BlockKitState>,
    cx: &App,
) -> Result<Json, String> {
    collect_modal_state_errors(ts, raw_blocks, kit, cx).map_err(|mut map| {
        let mut keys: Vec<String> = map.keys().cloned().collect();
        keys.sort();
        keys.first()
            .and_then(|k| map.remove(k))
            .unwrap_or_else(|| "Missing or invalid field.".to_owned())
    })
}

#[cfg(test)]
mod suggestion_race_tests {
    use super::*;

    #[test]
    fn external_select_suggestions_dedupe_and_drop_stale_responses() {
        let mut state = BlockKitState::default();
        let key = "modal:V1/b1/act1";

        let req1 = state
            .begin_suggestion_request(key, "alp")
            .expect("first request starts");
        assert!(req1.starts_with(key));
        // Duplicate rapid click with same query while in flight is suppressed.
        assert!(state.begin_suggestion_request(key, "alp").is_none());

        // User edits query before req1 completes -> supersedes with req2.
        let req2 = state
            .begin_suggestion_request(key, "alpha")
            .expect("new query supersedes in-flight request");
        assert_ne!(req1, req2);

        // Latest response lands first.
        state.set_suggestions(req2, vec![("Alpha".into(), "a".into())]);
        assert!(!state.suggestions_loading.contains(key));
        assert_eq!(
            state.suggestions.get(key),
            Some(&vec![("Alpha".into(), "a".into())])
        );

        // Stale out-of-order req1 arrives later and must be ignored.
        state.set_suggestions(req1, vec![("Stale".into(), "s".into())]);
        assert_eq!(
            state.suggestions.get(key),
            Some(&vec![("Alpha".into(), "a".into())])
        );

        // Unsequenced keys remain backwards-compatible.
        state.set_suggestions(key.to_owned(), vec![("Direct".into(), "d".into())]);
        assert_eq!(
            state.suggestions.get(key),
            Some(&vec![("Direct".into(), "d".into())])
        );
    }
}
