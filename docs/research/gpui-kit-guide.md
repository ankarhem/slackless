# GPUI Kit implementer guide (slackless)

Everything below was verified against a compiling, running spike:

- Spike: `spikes/gpui-kit-spike/` (standalone crate, own empty `[workspace]`).
- Deps: `gpui-kit = "0.10.0"` (which pins `gpui-pre-* 0.3.12`, the published GPUI
  snapshot), plus `tokio`, `futures`, and `pathfinder_simd` (workaround, see
  Build requirements).
- Toolchain: rust nightly 1.102.0-nightly (59cbdee2e 2026-11-17) from the repo's
  fenix dev shell, target aarch64-apple-darwin.
- Verification (2026-11-19):
  `CARGO_TARGET_DIR=<repo>/target CARGO_PROFILE_DEV_DEBUG=0 cargo build
--manifest-path spikes/gpui-kit-spike/Cargo.toml` exits 0 with zero warnings,
  and the binary was launched, ran ~12 s, printed nothing to stderr, no panic,
  then was killed. UI interactions were not manually clicked through; behavior
  claims below come from the gpui-kit 0.10.0 sources the spike links against.

All Rust snippets are copied verbatim from `spikes/gpui-kit-spike/src/main.rs`.

## 1. App initialization

`Cargo.toml`:

```toml
[workspace]

[package]
name = "gpui-kit-spike"
version = "0.1.0"
edition = "2021"

[dependencies]
gpui-kit = "0.10.0"
tokio = { version = "1", features = ["rt-multi-thread", "time"] }
futures = "0.3"
pathfinder_simd = { version = "0.5", features = ["pf-no-simd"] } # see Build requirements
```

Imports used by the whole spike (the `gpui` crate is reached through
`gpui_kit::gpui`, no separate dependency needed):

```rust
use gpui_kit::gpui;
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    actions, div, img, px, size, AppContext as _, Context, Entity, EventEmitter,
    FontWeight, Image, ImageFormat, ImageSource, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled, StyledText,
    Subscription, TextRun, Window, WindowBounds, WindowOptions,
};
use gpui_kit::assets::Assets;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{InputEvent, Textarea, TextareaState},
    label::Label,
    menu::ContextMenuExt as _,
    message_scroller::{MessageScroller, MessageScrollerState},
    notification::Notification,
    popover::Popover,
    ActiveTheme as _, Theme, ThemeMode, TitleBar, WindowExt as _, h_flex,
    h_resizable, resizable_panel, v_flex,
};
use gpui_kit::open_window;
```

Startup sequence:

```rust
let app = gpui_kit::application().with_assets(Assets);

app.run(move |cx| {
    gpui_kit::init(cx);

    cx.bind_keys([KeyBinding::new("cmd-k", OpenCommandPalette, None)]);

    // ...

    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1100.), px(760.)), cx)),
        ..Default::default()
    };

    open_window(window_options, cx, |window, cx| {
        // ... build entities, return the root view entity
    })
    .expect("Failed to open window");
});
```

- `gpui_kit::init(cx)` must run before any gpui-component feature (theme,
  dialogs, notifications, ...).
- `gpui_kit::application().with_assets(Assets)` bundles the default Lucide icon
  set (`gpui-kit-assets`); without it, icons do not resolve.
- `gpui_kit::open_window` always mounts a Base `Root` around your view, so
  dialogs, sheets, notifications and menus work without any extra wrapper.
  Do not add your own `Root`.
- Title bar: render `TitleBar::new().child("...")` as the first child of the
  root view. `TitleBar::window_options()` (seen in the kit examples) returns a
  `WindowOptions` preset for the custom title bar look; the spike instead uses
  plain `WindowOptions` with `WindowBounds::centered(size(..), cx)` (note:
  `size()` takes 2 args, `centered` takes the size **and** `&App`).
- Useful trait imports that are easy to forget (all required by the spike):
  `InteractiveElement` (`.id()`), `StatefulInteractiveElement` (`.on_click`),
  `prelude::FluentBuilder` (`.when`), `ScrollableElement`
  (`.overflow_y_scrollbar()`), `component::WindowExt`
  (`window.push_notification`, `window.open_dialog`, ...),
  `component::ActiveTheme` (`cx.theme()`), `button::ButtonVariants`
  (`.primary()`, `.ghost()`), `menu::ContextMenuExt` (`.context_menu`).

## 2. Entity/state model and subscriptions

State lives in `Entity<T>` values created with `cx.new`. Views implement
`Render`; child views render by passing the entity itself to `.child(...)`.
A model emits typed events with `EventEmitter` + `cx.emit`, and a parent
subscribes with `cx.subscribe` (app-level) or `cx.subscribe_in` (needs a
`Window`, used for input widgets). Keep the returned `Subscription` in a field,
or it is dropped and the subscription dies.

Model with typed event:

```rust
struct SidebarEvent;

struct Sidebar {
    sections: Vec<(&'static str, Vec<&'static str>)>,
    selected: usize, // flat index over all rows
}

impl EventEmitter<SidebarEvent> for Sidebar {}
```

Row click mutates the model, emits, and redraws:

```rust
.on_click(cx.listener(move |this, _, _, cx| {
    this.selected = ix;
    cx.emit(SidebarEvent);
    cx.notify();
}))
```

Root subscribing to the model (inside `cx.new(|cx| ...)` for the root view):

```rust
let subscriptions = vec![cx.subscribe(
    &sidebar,
    |_: &mut Spike, _, _: &SidebarEvent, cx| {
        cx.notify();
    },
)];
Spike { sidebar, chat, dark: true, _subscriptions: subscriptions }
```

Input subscription (needs the window; note the handler signature carries both
`window` and `cx`):

```rust
subscriptions.push(cx.subscribe_in(&editor, window, |this, _, event: &InputEvent, window, cx| {
    if let InputEvent::PressEnter { shift: false, .. } = event {
        this.send(window, cx);
    }
}));
```

Cross-entity updates use `entity.update(cx, |inner, cx| ...)`; reads use
`entity.read(cx)`. `cx.notify()` marks the current entity dirty and triggers a
re-render.

## 3. Resizable sidebar with sections and selectable rows

`h_resizable(id)` returns a `ResizablePanelGroup`; each child is a
`resizable_panel().child(element)`. The group has no `size_full()`, so wrap it
in a sized `div`. Dragging the divider between the two panels resizes them.

```rust
div().size_full().child(
h_resizable("main")
    .child(
        resizable_panel()
            .child(
                v_flex()
                    .h_full()
                    .w_64()
                    .bg(theme.secondary)
                    .child(self.sidebar.clone()),
            ),
    )
    .child(
        resizable_panel().child(
            v_flex()
                .size_full()
                // ... main column ...
                .child(self.chat.clone()),
        ),
    ),
)
```

Sections + selectable rows are plain styled divs (full source in the spike's
`impl Render for Sidebar`); the essential row pattern:

```rust
div()
    .id(SharedString::from(format!("channel-{name}")))
    .px_2()
    .py_1()
    .rounded_sm()
    .cursor_pointer()
    .text_color(if selected {
        cx.theme().foreground
    } else {
        cx.theme().muted_foreground
    })
    .when(selected, |el| {
        el.bg(cx.theme().secondary_active)
    })
    .hover(|el| el.bg(cx.theme().secondary_hover))
    .on_click(cx.listener(move |this, _, _, cx| {
        this.selected = ix;
        cx.emit(SidebarEvent);
        cx.notify();
    }))
    .child(format!("# {name}"))
```

`.id(...)` must be a stable `ElementId`; `(&str, &&str)` tuples do not
implement `Into<ElementId>` - use `SharedString::from(format!(...))` or the
supported tuple forms like `(&str, usize)`.

## 4. Variable-height virtualized message list (10,000 rows, bottom-anchored)

gpui-component ships `MessageScroller` / `MessageScrollerState`
(`component::message_scroller`), built on GPUI's measured `ListState` - rows are
measured individually, so heights may differ per row.

State construction (`FollowMode::Tail` is set inside
`MessageScrollerState::new`, which is why the list starts scrolled to the
bottom and follows new content):

```rust
let messages = (0..10_000).map(ChatMessage::variable).collect::<Vec<_>>();
// Constructor enables FollowMode::Tail, so the list starts at the bottom.
let scroller = cx.new(|cx| MessageScrollerState::new(10_000, cx));
```

Rendering. The row renderer must be `'static`, so it cannot borrow the view.
Keep the data in its own `Entity` and capture a clone:

```rust
/// Message data lives in its own entity so the 'static virtual-list row
/// renderer can read it through an `Entity` clone.
struct MessageStore {
    messages: Vec<ChatMessage>,
    avatar: Arc<Image>,
}
```

```rust
MessageScroller::new("messages", scroller, move |ix, _window, cx| {
    let store = store.read(cx);
    let msg = &store.messages[ix.min(store.messages.len() - 1)];
    let avatar = store.avatar.clone();
    h_flex()
        .items_start()
        .gap_3()
        .px_4()
        .py_3()
        .child(
            img(ImageSource::Image(avatar))
                .size(px(36.))
                .rounded_full(),
        )
        .child(div().flex_1().child(render_message(msg, cx)))
})
.jump_button(true)
```

Appending while staying glued to the bottom (tail following does the
anchoring; no manual scroll math):

```rust
self.store.update(cx, |store, _| {
    store.messages.push(ChatMessage { author: "me", body: text });
});
// Tail following keeps the bottom anchored after the append.
self.scroller.update(cx, |s, cx| {
    s.append(1, cx);
});
```

`MessageScrollerState` also has `prepend(count, cx)` (history backfill that
preserves the scroll anchor), `splice`, `remeasure_items`, `scroll_to_item`,
`scroll_to_offset`, `is_scrolled_up`, `is_following_tail`, and the built-in
"Jump to latest" button + scrollbar are on by default (`.jump_button(true)`
shown above). This is exactly the Slack history API: prepend for pagination,
append for live messages, splice for edits/deletes.

## 5. Multi-line auto-growing composer (Enter sends, Shift+Enter newline)

`TextareaState` + `Textarea` from `component::input`. Default behavior is
already Slack's: **Enter emits `InputEvent::PressEnter { shift: false, .. }`
without inserting a newline; Shift+Enter inserts a newline** (there is also a
`submit_on_enter(bool)` builder on the base state if you ever need to change
it).

```rust
let editor = cx.new(|cx| {
    TextareaState::new(window, cx)
        .placeholder("Message #general — Enter sends, Shift+Enter newline")
        .auto_grow(1, 8)
});
```

```rust
.child(div().flex_1().child(Textarea::new(&self.editor).appearance(true)))
```

Read and clear the value:

```rust
let text = self.editor.read(cx).value().to_string();
if text.trim().is_empty() {
    return;
}
// ...
self.editor.update(cx, |s, cx| {
    s.set_value("", window, cx);
});
```

`set_value(value, window, cx)` takes the `&mut Window` by reference - no clone.

## 6. Right-click context menu and popover

Context menu: `ContextMenuExt::context_menu` on any interactive element with a
stable id. Menu items dispatch `Action`s (declared via `actions!`).

```rust
actions!(spike, [OpenCommandPalette, CopyLink]);
```

```rust
.id("header")
.context_menu(move |menu, _, _| {
    menu.menu(
        "Mark as read",
        Box::new(OpenCommandPalette),
    )
    .separator()
    .menu("Copy link", Box::new(CopyLink))
})
```

(There is no `menu_with_click`; free-form items are built with
`PopupMenuItem::new(label).on_click(...)` if you don't want an action.)

Popover: `Popover::new(id).trigger(element).content(closure)`; the content
closure receives `(&mut PopoverState, &mut Window, &mut Context<PopoverState>)`.

```rust
Popover::new("info-popover")
    .trigger(
        Button::new("info")
            .ghost()
            .label("Info"),
    )
    .content(|_, _, _| {
        v_flex()
            .p_3()
            .gap_1()
            .child(Label::new("About this channel"))
            .child(Label::new("A popover from gpui-kit"))
    })
```

## 7. Modal dialog

`WindowExt::open_dialog` (requires the `Root` that `open_window` mounts):

```rust
window.open_dialog(cx, |dialog, _, _| {
    dialog
        .title("Modal dialog")
        .child(
            v_flex().p_4().child(Label::new(
                "A modal from window.open_dialog",
            )),
        )
});
```

`window.open_sheet` exists with the same shape for side sheets.

## 8. Toast notification

`WindowExt::push_notification` with `component::notification::Notification`:

```rust
window.push_notification(
    Notification::new()
        .title("Saved")
        .message("Toast notification works")
        .autohide(true),
    cx,
);
```

The same `Notification` supports `NotificationDelivery::System` /
`InAppAndSystem` to post to the macOS notification center through GPUI's
`SystemNotification` platform API (see Platform capabilities).

## 9. Image from in-memory bytes

Slack file URLs need cookie-authenticated downloads, so the render path must
accept bytes, not URLs. `gpui::Image::from_bytes(format, Vec<u8>)` +
`ImageSource::Image` + `img(...)` does exactly that:

```rust
let avatar = Arc::new(Image::from_bytes(
    ImageFormat::Png,
    include_bytes!("assets/avatar.png").to_vec(),
));
```

```rust
img(ImageSource::Image(avatar))
    .size(px(36.))
    .rounded_full()
```

In slackless, download with the session cookie in the async worker, then hand
`Vec<u8>` to this path (decoded once, cached, shared as `Arc<Image>`).

## 10. Styled inline text runs (bold, italic, code, mention chip, link)

`StyledText::new(text).with_runs(Vec<TextRun>)`. In `gpui-pre 0.3.12`,
`TextRun` has exactly these fields - weight and slant live on `Font`, and
there is **no** per-run link field:

```rust
TextRun {
    len: "bold".len(),                       // utf-8 byte length of the run
    font: gpui::Font { weight: FontWeight::BOLD, ..base_font.clone() },
    color: theme.foreground,
    background_color: None,
    underline: Default::default(),
    strikethrough: Default::default(),
}
```

Italic:

```rust
TextRun {
    len: " italic".len(),
    font: gpui::Font { style: gpui::FontStyle::Italic, ..base_font.clone() },
    color: theme.foreground,
    background_color: None,
    underline: Default::default(),
    strikethrough: Default::default(),
}
```

Inline code (mono font + background):

```rust
TextRun {
    len: " code ".len(),
    font: mono_font.clone(), // gpui::Font { family: theme.mono_font_family.clone(), ..base_font.clone() }
    color: theme.foreground,
    background_color: Some(code_bg), // theme.secondary
    underline: Default::default(),
    strikethrough: Default::default(),
}
```

Highlighted @mention chip (accent background + link color):

```rust
TextRun {
    len: "@alice".len(),
    font: gpui::Font { weight: FontWeight::SEMIBOLD, ..base_font.clone() },
    color: theme.link,
    background_color: Some(mention_bg), // theme.accent.opacity(0.25)
    underline: Default::default(),
    strikethrough: Default::default(),
}
```

Base font and assembly:

```rust
let base_font = gpui::Font { family: theme.font_family.clone(), ..Default::default() };
// ...
StyledText::new("bold italic code @alice").with_runs(runs)
```

Runs must cover the whole string in order (GPUI warns if they don't). For a
clickable link use the `Link` component next to the styled text:

```rust
gpui_kit::component::link::Link::new("docs")
    .href("https://gpui-kit.com")
    .child("Docs")
```

Note for slackless: links inside a `StyledText` paragraph are not clickable per
run; either segment the message into `Link` children (as above), or use
`component::text::TextView::markdown(...)` which renders markdown/HTML with
link handling for free.

## 11. Light/dark theme toggle

```rust
Theme::change(
    if this.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    },
    None,
    cx,
);
cx.notify();
```

Read the active palette anywhere via `cx.theme()` (`ActiveTheme` trait);
fields used in the spike: `background`, `foreground`, `muted_foreground`,
`secondary`, `secondary_hover`, `secondary_active`, `border`, `accent`,
`link`, `font_family`, `mono_font_family` (there is no `text_muted` field -
it's `muted_foreground`). `ThemeMode` also has a `System` variant.

## 12. Keybinding bound to an action (Cmd+K)

Declare the action, bind the key at app start, handle it on a focusable
element in the view tree:

```rust
actions!(spike, [OpenCommandPalette, CopyLink]);
```

```rust
cx.bind_keys([KeyBinding::new("cmd-k", OpenCommandPalette, None)]);
```

```rust
div()
    .id("root")
    // ...
    .on_action(cx.listener(|_, _: &OpenCommandPalette, window, cx| {
        // Cmd+K lands here.
        window.push_notification(
            Notification::new()
                .title("Command palette")
                .message("Cmd+K action dispatched")
                .autohide(true),
            cx,
        );
    }))
```

The third `KeyBinding::new` argument is an optional context predicate string
(e.g. `Some("Editor")`) for scoped bindings.

## 13. Async work: `cx.spawn`, background executor, and tokio interop

GPUI's async runtime is **smol-based, not tokio**. `cx.spawn(async move |this,
cx| ...)` runs a future on it and gives you a `WeakEntity`-style handle back
into your view (`this.update(cx, |view, cx| ...)`). A dropped `Task` keeps
running detached. `cx.background_executor()` (from `AppContext`) exposes the
same executor for CPU/IO-bound futures and timers.

A tokio-based HTTP/websocket client (reqwest, tokio-tungstenite, ...) cannot
run on that executor. The working pattern, compiled and run in the spike: a
**dedicated OS thread owning a tokio runtime**, with results pushed through a
`futures::channel::mpsc` channel that a `cx.spawn` task drains into entities.
(`futures` channels are runtime-agnostic, so the tokio side can `send` and the
smol side can `await` on the same channel.)

Producer (called once in `app.run`):

```rust
let (tx, mut rx) = futures::channel::mpsc::unbounded::<String>();
std::thread::spawn(move || {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            // Stand-in for a websocket frame / HTTP response.
            if tx
                .unbounded_send("async worker tick received".to_string())
                .is_err()
            {
                break;
            }
        }
    });
});
```

Consumer (inside `open_window`, after creating the root entity):

```rust
spike.update(cx, |_, cx| {
    // A dropped gpui Task keeps running (detached).
    let _task = cx.spawn(async move |this, cx| {
        while let Some(msg) = rx.next().await {
            let _ = this.update(cx, |spike: &mut Spike, cx| {
                let _ = (&spike.dark, &msg);
                cx.notify();
            });
        }
    });
});
```

For slackless: run the whole Slack client (cookie-authenticated REST +
websocket) on that tokio thread, send commands in over another channel, and
translate incoming events into entity updates in the drain loop. Verified: the
pattern compiles and the app runs with the runtime thread live; the 30 s tick
itself was not observed within the 12 s launch check.

## 14. macOS platform capabilities (dock badge, notifications, status bar)

Checked against the pinned `gpui-pre-* 0.3.12` sources:

- **User notifications: yes.** GPUI has a `SystemNotification` platform API
  (`show_system_notification`, `on_system_notification_response`,
  `SystemNotificationAction`), and gpui-component's `Notification` can deliver
  via `NotificationDelivery::System` / `InAppAndSystem` out of the box.
- **Dock badge: no.** No dock-tile/badge API exists anywhere in gpui-pre or
  gpui-kit. Unread-count badges need platform code via `objc2`
  (`NSApplication.dockTile.badgeLabel`), same as Zed does.
- **Status-bar item: no.** No `NSStatusItem` support; needs `objc2` platform
  code as well.

## 15. macOS build requirements (as hit in this Nix shell)

- **Metal shaders: no full Xcode needed.** In this pinned snapshot,
  `gpui-pre-platform` enables its `runtime_shaders` feature **by default**
  (verified with `cargo tree -e features`: `gpui-pre-platform feature
"runtime_shaders"` -> `gpui-pre-macos` -> `gpui-pre-apple`). With that
  feature, `gpui-pre-apple/build.rs` stitches shader sources
  (`stitched_shaders.metal`) instead of invoking `xcrun metal`, and shaders
  compile on the GPU at runtime. This shell's `xcrun` is the xcbuild shim
  (`xcrun --find metal` -> `error: tool 'metal' not found`) and the build still
  succeeded. If you ever disable default features, shader compilation falls
  back to `xcrun -sdk macosx metal`, which needs full Xcode.
- **SDK/linker:** the flake's `DEVELOPER_DIR`/`SDKROOT` pointing at apple-sdk
  14.4 plus gpui-pre's build scripts (framework links emitted by the crates)
  were sufficient; no manual `-framework` flags were added.
- **Nightly rustc ICE workaround (required).** The first build failed inside a
  dependency with an internal compiler error:

  ```
  thread 'rustc' panicked at .../compiler/rustc_monomorphize/src/mono_checks/abi_check.rs:302:21
  query stack during panic:
  #0 [check_mono_item] monomorphization-time checking
  #1 [items_of_instance] collecting items used by `pathfinder_simd::arm::F32x4::abs`
  error: could not compile `pathfinder_geometry` (lib)
  note: rustc 1.102.0-nightly (59cbdee2e 2026-11-17) running on aarch64-apple-darwin
  ```

  `pathfinder_geometry` is a hard dependency of `gpui-pre-apple` (also via
  `zed-font-kit`), and its `pathfinder_simd` dep enables a nightly-only
  aarch64 SIMD module (`std::intrinsics::simd`) whenever it detects a nightly
  compiler. Tried and working: force the scalar fallback by enabling the
  `pf-no-simd` feature crate-wide through a direct dependency (cargo feature
  unification applies it to the whole graph):

  ```toml
  pathfinder_simd = { version = "0.5", features = ["pf-no-simd"] }
  ```

  After this, the entire graph compiled and linked. Cost: GPUI's font/vector
  math uses scalar instead of NEON paths on aarch64; fine for development.

- **Disk:** the dev profile with `CARGO_PROFILE_DEV_DEBUG=0` and a shared
  `CARGO_TARGET_DIR=<repo>/target` used ~1 GB. No release builds were made.

## 16. Component gaps a Slack clone must build itself

What gpui-kit gives you for free: the message list, composer, sidebar
scaffolding, dialogs/sheets/popovers/menus/toasts, themes, markdown/HTML
rendering (`TextView`), badges, avatars, dock layout, tables, code editor.

What it does not give you (build these):

- **Slack-flavored message rendering pipeline**: mrkdwn parsing into
  `TextRun`s, per-run links/mention chips that react to clicks, emoji
  shortcode substitution and custom-emoji images inline. `StyledText` runs
  can't embed arbitrary child elements, so complex messages need either
  segmented children or custom elements.
- **Composer extras**: @mention/#channel autocomplete (the textarea has token
  and popover infrastructure, but the Slack data source and UI are yours),
  emoji picker, formatting toolbar, draft persistence per channel.
- **File handling**: cookie-authenticated image downloader + `Arc<Image>`
  cache (Section 9 covers only rendering), file upload with progress, video /
  audio players, file cards.
- **Chat behaviors**: unread markers and divider lines, typing indicators,
  presence dots, reaction chips with pickers, message editing in place,
  threads pane (the dock/resizable system can host it, the logic is yours).
- **History/backfill orchestration**: `MessageScrollerState::prepend` keeps
  the anchor, but deciding when to fetch older pages and merging
  edits/deletes via `splice` is application logic.
- **Workspace switching, auth/session storage** (Keychain via `security`/
  objc2), and the macOS integrations listed above (dock badge, status bar).
