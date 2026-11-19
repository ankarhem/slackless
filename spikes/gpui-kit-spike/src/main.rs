//! GPUI Kit spike for slackless: demonstrates the exact building blocks a
//! Slack clone needs. See docs/research/gpui-kit-guide.md for the write-up.

use std::{sync::Arc, time::Duration};

use futures::StreamExt as _;
use gpui_kit::assets::Assets;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Theme, ThemeMode, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, h_resizable,
    input::{InputEvent, Textarea, TextareaState},
    label::Label,
    menu::ContextMenuExt as _,
    message_scroller::{MessageScroller, MessageScrollerState},
    notification::Notification,
    popover::Popover,
    resizable_panel, v_flex,
};
use gpui_kit::gpui;
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, Image, ImageFormat, ImageSource,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled, StyledText, Subscription, TextRun, Window,
    WindowBounds, WindowOptions, actions, div, img, px, size,
};
use gpui_kit::open_window;

actions!(spike, [OpenCommandPalette, CopyLink]);

// ---------------------------------------------------------------------------
// Sidebar model: sections + selectable rows, emits a typed event on select.
// ---------------------------------------------------------------------------

struct SidebarEvent;

struct Sidebar {
    sections: Vec<(&'static str, Vec<&'static str>)>,
    selected: usize, // flat index over all rows
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut flat = 0usize;
        let mut col = v_flex().flex_1().gap_1().p_2().overflow_y_scrollbar();
        for (section, channels) in &self.sections {
            col = col
                .child(
                    div()
                        .id(SharedString::from(format!("section-{section}")))
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .text_size(xs())
                        .text_color(cx.theme().muted_foreground)
                        .child(*section),
                )
                .map(|col| {
                    let mut sec = v_flex().gap_0p5();
                    for name in channels {
                        let ix = flat;
                        flat += 1;
                        let selected = ix == self.selected;
                        sec = sec.child(
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
                                .when(selected, |el| el.bg(cx.theme().secondary_active))
                                .hover(|el| el.bg(cx.theme().secondary_hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected = ix;
                                    cx.emit(SidebarEvent);
                                    cx.notify();
                                }))
                                .child(format!("# {name}")),
                        );
                    }
                    col.child(sec)
                });
        }
        col
    }
}

fn xs() -> gpui::Rems {
    gpui::rems(0.75)
}

// ---------------------------------------------------------------------------
// Chat state: 10,000 variable-height messages in a virtualized list that
// follows its tail, plus the composer.
// ---------------------------------------------------------------------------

struct ChatMessage {
    author: &'static str,
    body: String,
}

impl ChatMessage {
    fn variable(i: usize) -> Self {
        // Rows deliberately differ in length so wrapped heights differ.
        let filler = match i % 5 {
            0 => "hey!",
            1 => {
                "This line is long enough to wrap onto multiple lines in the message list when the window is at a normal width, which is exactly what a Slack message does all day long."
            }
            2 => "shipping the thing :tada:",
            3 => {
                "one more wrap-width line to make the virtual list measure plenty of distinct row heights during scrolling, good for the spike"
            }
            _ => "ok",
        };
        Self {
            author: AUTHORS[i % AUTHORS.len()],
            body: filler.to_string(),
        }
    }
}

const AUTHORS: [&str; 3] = ["alice", "bob", "carol"];

/// Renders one message row with styled inline runs: bold, italic, inline code,
/// a highlighted @mention chip. A clickable link uses the Link component.
fn render_message(msg: &ChatMessage, cx: &gpui::App) -> impl IntoElement {
    let theme = cx.theme();
    let code_bg = theme.secondary;
    let mention_bg = theme.accent.opacity(0.25);

    let base_font = gpui::Font {
        family: theme.font_family.clone(),
        ..Default::default()
    };
    let body_font = gpui::Font {
        weight: FontWeight::BOLD,
        ..base_font.clone()
    };
    let runs = vec![TextRun {
        len: msg.body.len(),
        font: body_font,
        color: theme.foreground,
        background_color: None,
        underline: Default::default(),
        strikethrough: Default::default(),
    }];

    v_flex()
        .gap_0p5()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .text_size(xs())
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.link)
                        .child(msg.author),
                )
                .child(
                    div()
                        .text_size(xs())
                        .text_color(theme.muted_foreground)
                        .child("10:0"),
                ),
        )
        // The message body: the long body keeps variable row heights; a short
        // decorated sample line shows the styled runs.
        .child(StyledText::new(msg.body.clone()).with_runs(runs))
        .when(msg.body == "hey!", |el| {
            // Inline decorated sample: **bold**, *italic*, `code`, @mention.
            let sample = "bold italic code @alice";
            let mono_font = gpui::Font {
                family: theme.mono_font_family.clone(),
                ..base_font.clone()
            };
            let runs = vec![
                TextRun {
                    len: "bold".len(),
                    font: gpui::Font {
                        weight: FontWeight::BOLD,
                        ..base_font.clone()
                    },
                    color: theme.foreground,
                    background_color: None,
                    underline: Default::default(),
                    strikethrough: Default::default(),
                },
                TextRun {
                    len: " italic".len(),
                    font: gpui::Font {
                        style: gpui::FontStyle::Italic,
                        ..base_font.clone()
                    },
                    color: theme.foreground,
                    background_color: None,
                    underline: Default::default(),
                    strikethrough: Default::default(),
                },
                TextRun {
                    len: " code ".len(),
                    font: mono_font.clone(),
                    color: theme.foreground,
                    background_color: Some(code_bg),
                    underline: Default::default(),
                    strikethrough: Default::default(),
                },
                TextRun {
                    len: "@alice".len(),
                    font: gpui::Font {
                        weight: FontWeight::SEMIBOLD,
                        ..base_font.clone()
                    },
                    color: theme.link,
                    background_color: Some(mention_bg),
                    underline: Default::default(),
                    strikethrough: Default::default(),
                },
            ];
            let _ = sample;
            el.child(
                h_flex()
                    .gap_1()
                    .mt_1()
                    .child(StyledText::new("bold italic code @alice").with_runs(runs))
                    .child(
                        // Clickable link component.
                        gpui_kit::component::link::Link::new("docs")
                            .href("https://gpui-kit.com")
                            .child("Docs"),
                    ),
            )
        })
}

/// Message data lives in its own entity so the 'static virtual-list row
/// renderer can read it through an `Entity` clone.
struct MessageStore {
    messages: Vec<ChatMessage>,
    /// In-memory image rendered from bytes (cookie-auth downloads means Slack
    /// file images can never come from a URL fetch at render time).
    avatar: Arc<Image>,
}

struct Chat {
    store: Entity<MessageStore>,
    scroller: Entity<MessageScrollerState>,
    editor: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl Chat {
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.store.update(cx, |store, _| {
            store.messages.push(ChatMessage {
                author: "me",
                body: text,
            });
        });
        // Tail following keeps the bottom anchored after the append.
        self.scroller.update(cx, |s, cx| {
            s.append(1, cx);
        });
        self.editor.update(cx, |s, cx| {
            s.set_value("", window, cx);
        });
        cx.notify();
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let messages = (0..10_000).map(ChatMessage::variable).collect::<Vec<_>>();
        // Constructor enables FollowMode::Tail, so the list starts at the bottom.
        let scroller = cx.new(|cx| MessageScrollerState::new(10_000, cx));
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Message #general — Enter sends, Shift+Enter newline")
                .auto_grow(1, 8)
        });
        let avatar = Arc::new(Image::from_bytes(
            ImageFormat::Png,
            include_bytes!("assets/avatar.png").to_vec(),
        ));
        let store = cx.new(|_| MessageStore { messages, avatar });

        let mut subscriptions = Vec::new();
        // Default textarea behavior: Enter emits PressEnter without inserting a
        // newline; Shift+Enter inserts one. Exactly Slack's composer semantics.
        subscriptions.push(cx.subscribe_in(
            &editor,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send(window, cx);
                }
            },
        ));

        Self {
            store,
            scroller,
            editor,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for Chat {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scroller = self.scroller.clone();
        let store = self.store.clone();

        v_flex()
            .flex_1()
            .size_full()
            .child(
                div().flex_1().min_h_0().child(
                    MessageScroller::new("messages", scroller, move |ix, _window, cx| {
                        let store = store.read(cx);
                        let msg = &store.messages[ix.min(store.messages.len() - 1)];
                        let avatar = store.avatar.clone();
                        h_flex()
                            .items_start()
                            .gap_3()
                            .px_4()
                            .py_3()
                            .child(img(ImageSource::Image(avatar)).size(px(36.)).rounded_full())
                            .child(div().flex_1().child(render_message(msg, cx)))
                    })
                    .jump_button(true),
                ),
            )
            .child(
                // Composer.
                h_flex()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .child(Textarea::new(&self.editor).appearance(true)),
                    )
                    .child(
                        Button::new("send")
                            .primary()
                            .label("Send")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.send(window, cx);
                            })),
                    ),
            )
    }
}

// ---------------------------------------------------------------------------
// Root view
// ---------------------------------------------------------------------------

struct Spike {
    sidebar: Entity<Sidebar>,
    chat: Entity<Chat>,
    dark: bool,
    _subscriptions: Vec<Subscription>,
}

impl Render for Spike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let dark = self.dark;

        div()
            .id("root")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
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
            .child(TitleBar::new().child("slackless — GPUI Kit spike"))
            .child(
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
                                .child(
                                    // Header: popover + toast + dialog + theme buttons,
                                    // with a context menu on right-click.
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .px_4()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(theme.border)
                                        .id("header")
                                        .context_menu(move |menu, _, _| {
                                            menu.menu(
                                                "Mark as read",
                                                Box::new(OpenCommandPalette),
                                            )
                                            .separator()
                                            .menu("Copy link", Box::new(CopyLink))
                                        })
                                        .child(Label::new("#general"))
                                        .child(
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
                                                }),
                                        )
                                        .child(
                                            Button::new("toast")
                                                .ghost()
                                                .label("Toast")
                                                .on_click(|_, window, cx| {
                                                    window.push_notification(
                                                        Notification::new()
                                                            .title("Saved")
                                                            .message("Toast notification works")
                                                            .autohide(true),
                                                        cx,
                                                    );
                                                }),
                                        )
                                        .child(
                                            Button::new("dialog")
                                                .ghost()
                                                .label("Dialog")
                                                .on_click(|_, window, cx| {
                                                    window.open_dialog(cx, |dialog, _, _| {
                                                        dialog
                                                            .title("Modal dialog")
                                                            .child(
                                                                v_flex().p_4().child(Label::new(
                                                                    "A modal from window.open_dialog",
                                                                )),
                                                            )
                                                    });
                                                }),
                                        )
                                        .child(
                                            Button::new("theme")
                                                .ghost()
                                                .label(if dark { "Light" } else { "Dark" })
                                                .on_click(cx.listener(
                                                    |this, _: &gpui::ClickEvent, _, cx| {
                                                        this.dark = !this.dark;
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
                                                    },
                                                )),
                                        ),
                                )
                                .child(self.chat.clone()),
                        ),
                    ),
                ),
            )
    }
}

fn main() {
    let app = gpui_kit::application().with_assets(Assets);

    app.run(move |cx| {
        gpui_kit::init(cx);

        cx.bind_keys([KeyBinding::new("cmd-k", OpenCommandPalette, None)]);

        // -- Async pattern: a tokio runtime on its own thread, results carried
        // into gpui through a channel drained by cx.spawn. gpui's own async
        // runtime is smol-based; tokio-based HTTP/WS clients cannot run on it.
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

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1100.), px(760.)), cx)),
            ..Default::default()
        };

        open_window(window_options, cx, |window, cx| {
            let sidebar = cx.new(|_| Sidebar {
                sections: vec![
                    ("Channels", vec!["general", "engineering", "random"]),
                    ("Direct messages", vec!["alice", "bob", "carol"]),
                ],
                selected: 0,
            });

            let chat = cx.new(|cx| Chat::new(window, cx));

            let spike = cx.new(|cx| {
                // Subscribe the root to the sidebar model's typed events.
                let subscriptions =
                    vec![
                        cx.subscribe(&sidebar, |_: &mut Spike, _, _: &SidebarEvent, cx| {
                            cx.notify();
                        }),
                    ];
                Spike {
                    sidebar,
                    chat,
                    dark: true,
                    _subscriptions: subscriptions,
                }
            });
            // Drain the tokio worker channel into the app (dropped Task = detached).
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
            spike
        })
        .expect("Failed to open window");
    });
}
