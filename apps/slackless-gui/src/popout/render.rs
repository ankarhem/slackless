//! Window chrome, overlay layers, and per-kind body renderers (`Conversation`, `Thread`,
//! `Activity`, `Later`, `Canvas`, `Files`) for popped-out secondary GPUI windows.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, App, Context, Edges, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    anchored, deferred, div, px,
};
use slackless_core::types::{ChannelId, Message};
use slackless_core::{CollabCommand, WorkspaceCommand};

use crate::actions::{self, ActionHandler, ActionSet, MenuOrigin};
use crate::messages::{LinkHandler, message_row_linked, reply_bar, same_group};

use super::{JumpHandler, PopoutKind, PopoutWindow};

impl PopoutWindow {
    pub(super) fn render_overlays(
        &self,
        root: gpui_kit::gpui::Stateful<gpui_kit::gpui::Div>,
        viewer_top: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let picker_overlay = self
            .picker
            .as_ref()
            .map(|(_, pos)| *pos)
            .zip(self.emoji_picker.clone());
        root.when_some(picker_overlay, |root, (pos, emoji_picker)| {
            let viewport = window.viewport_size();
            let anchor = match (pos.y > viewport.height / 2., pos.x > viewport.width / 2.) {
                (true, true) => Anchor::BottomRight,
                (true, false) => Anchor::BottomLeft,
                (false, true) => Anchor::TopRight,
                (false, false) => Anchor::TopLeft,
            };
            root.child(
                div()
                    .id("popout-picker-backdrop")
                    .occlude()
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.picker = None;
                            cx.notify();
                        }),
                    ),
            )
            .child(deferred(
                anchored()
                    .position(pos)
                    .anchor(anchor)
                    .snap_to_window_with_margin(Edges::all(px(8.)))
                    .child(
                        div()
                            .id("popout-picker-overlay")
                            .occlude()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .child(emoji_picker),
                    ),
            ))
        })
        .when_some(self.image_viewer.clone(), |root, viewer| {
            root.child(
                div()
                    .id("popout-image-viewer-overlay")
                    .occlude()
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .absolute()
                    .top(viewer_top)
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(viewer),
            )
        })
    }

    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (bg, border, muted, fg) = (
            theme.background,
            theme.border,
            theme.muted_foreground,
            theme.foreground,
        );
        let kind_label = match &self.kind {
            PopoutKind::Conversation { .. } => "Conversation",
            PopoutKind::Thread { .. } => "Thread",
            PopoutKind::Activity { .. } => "Activity · Live",
            PopoutKind::Later { .. } => "Later · Live",
            PopoutKind::Canvas { .. } => "Canvas",
            PopoutKind::Files { .. } => "Files · Live",
            PopoutKind::Huddle { .. } => "Huddle · Live",
        };
        TitleBar::new()
            .h(px(40.))
            .bg(bg)
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pr_3()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(fg)
                                    .child(self.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(kind_label),
                            ),
                    )
                    .child(
                        Button::new("popout-refresh")
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::RefreshCw)
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.reload_from_store(cx))),
                    ),
            )
    }

    pub(super) fn render_messages_body(
        &self,
        channel: ChannelId,
        on_jump: JumpHandler,
        is_conversation: bool,
        links: &LinkHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let theme = cx.theme();
        let (bg, hover_bg, saved_bg) = (
            theme.background,
            theme.secondary.opacity(0.5),
            theme.warning.opacity(0.08),
        );
        let view = cx.entity();
        let mut rows: Vec<gpui_kit::gpui::AnyElement> = Vec::with_capacity(self.messages.len());
        let mut prev_msg: Option<&Message> = None;
        let editing = self.editing.clone();
        for (ix, msg) in self.messages.iter().enumerate() {
            if let Some((_, edit_ts, edit_input)) = &editing
                && *edit_ts == msg.ts
            {
                let edit_row = v_flex()
                    .id(SharedString::from(format!("popout-edit-{ix}")))
                    .mx_4()
                    .my_1()
                    .p_2p5()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().ring)
                    .bg(bg)
                    .child(Textarea::new(edit_input).appearance(false).bordered(false))
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("popout-edit-cancel")
                                    .outline()
                                    .with_size(Size::XSmall)
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.editing = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("popout-edit-save")
                                    .primary()
                                    .with_size(Size::XSmall)
                                    .label("Save Changes")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save_edit(cx);
                                    })),
                            ),
                    );
                rows.push(edit_row.into_any_element());
                prev_msg = None;
                continue;
            }
            let msg_group = SharedString::from(format!("popout-msg-{ix}"));
            let key = format!("popout-row-{}", msg.ts.as_str());
            let grouped = prev_msg
                .filter(|prev| same_group(prev, msg))
                .map(|_| msg_group.clone());
            let is_saved = self.saved.contains(&msg.ts);
            let view_for_action = view.clone();
            let ch_for_action = channel.clone();
            let msg_for_action = msg.clone();
            let handler: ActionHandler = Rc::new(move |action, window, cx| {
                let ch = ch_for_action.clone();
                let m = msg_for_action.clone();
                view_for_action.update(cx, |this, cx| {
                    this.handle_message_action(ch, m, action, window, cx);
                });
            });
            let set = ActionSet {
                own: msg.user.as_ref() == Some(&self.fixtures.self_user),
                reply: is_conversation,
                saved: is_saved,
                pinned: msg.pinned_to.contains(&channel),
                in_thread: !is_conversation
                    && matches!(&self.kind, PopoutKind::Thread { thread_ts, .. } if msg.ts != *thread_ts),
            };
            let mut display = msg.clone();
            let reactions = std::mem::take(&mut display.reactions);
            let chips =
                actions::reaction_chips(&key, &reactions, &self.fixtures, Some(&handler), cx)
                    .map(|c| c.pl(px(48.)));
            let row_body = message_row_linked(
                &self.fixtures,
                &display,
                Some(&handler),
                Some(links),
                grouped,
                cx,
            );
            let reply_bar_el = if is_conversation {
                let jump = on_jump.clone();
                let ch = channel.clone();
                let ts = msg.ts.clone();
                let reply_group = SharedString::from(format!("popout-reply-{ix}"));
                reply_bar(
                    &self.fixtures,
                    msg,
                    ("popout-thread-chip", ix),
                    msg_group.clone(),
                    reply_group,
                    move |_, cx| jump(ch.clone(), Some(ts.clone()), true, cx),
                    cx,
                )
                .map(|bar| div().pl(px(48.)).child(bar))
            } else {
                None
            };
            let quick =
                crate::preferences::active_quick_reactions(&self.fixtures.quick_reactions, cx);
            let menu_handler = handler.clone();
            let row_key = key.clone();
            let keep_inside = ix == 0;
            let item = actions::HoverOverlay::new(SharedString::from(key), move |hover, _, cx| {
                let revealed = hover.revealed(cx);
                v_flex()
                    .id(SharedString::from(row_key.clone()))
                    .group(msg_group)
                    .relative()
                    .px_4()
                    .py_0p5()
                    .when(is_saved, |row| {
                        row.bg(saved_bg).child(actions::saved_banner(cx))
                    })
                    .when(!is_saved && revealed, |row| row.bg(hover_bg))
                    .child(row_body)
                    .children(chips)
                    .children(reply_bar_el)
                    .when(revealed, |row| {
                        row.child(
                            actions::hover_toolbar(&row_key, &quick, set, &handler, &hover, cx)
                                .when(keep_inside, |bar| bar.top(px(3.))),
                        )
                    })
                    .on_hover(hover.host_on_hover())
                    .context_menu(move |menu, window, cx| {
                        actions::actions_menu(
                            menu,
                            set,
                            &menu_handler,
                            MenuOrigin::Context,
                            window,
                            cx,
                        )
                    })
                    .into_any_element()
            });
            rows.push(item.into_any_element());
            prev_msg = Some(msg);
        }
        v_flex()
            .id("popout-messages")
            .flex_1()
            .min_h_0()
            .py_3()
            .gap_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(rows)
    }

    pub(super) fn render_activity_body(
        &self,
        on_jump: JumpHandler,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let theme = cx.theme();
        let (bg, border, muted, accent) = (
            theme.background,
            theme.border,
            theme.muted_foreground,
            theme.accent,
        );
        let mut list = v_flex()
            .id("popout-activity")
            .flex_1()
            .min_h_0()
            .px_4()
            .py_3()
            .gap_2()
            .overflow_y_scroll();
        if self.activity_rows.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .text_sm()
                    .text_color(muted)
                    .child("No activity items."),
            );
        } else {
            list = list.children(self.activity_rows.iter().enumerate().map(|(ix, item)| {
                let jump = on_jump.clone();
                let channel = item.channel.clone();
                let thread_ts = item.thread_ts.clone();
                let is_thread = thread_ts.is_some();
                v_flex()
                    .id(SharedString::from(format!("popout-act-{ix}")))
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(if item.unread { accent.opacity(0.2) } else { bg })
                    .cursor_pointer()
                    .hover(|el| el.bg(accent.opacity(0.28)))
                    .on_click(move |_, _, cx| {
                        jump(channel.clone(), thread_ts.clone(), is_thread, cx);
                    })
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(item.title.clone()),
                    )
                    .child(div().text_sm().child(item.body.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(item.context.clone()),
                    )
            }));
        }
        list
    }

    pub(super) fn render_later_body(
        &self,
        on_jump: JumpHandler,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let theme = cx.theme();
        let (border, muted, accent) = (theme.border, theme.muted_foreground, theme.accent);
        let mut list = v_flex()
            .id("popout-later")
            .flex_1()
            .min_h_0()
            .px_4()
            .py_3()
            .gap_2()
            .overflow_y_scroll();
        if self.later_items.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .text_sm()
                    .text_color(muted)
                    .child("No saved items."),
            );
        } else {
            list = list.children(self.later_items.iter().enumerate().map(|(ix, item)| {
                let jump = on_jump.clone();
                let channel = item.channel.clone();
                let ts = item.ts.clone();
                v_flex()
                    .id(SharedString::from(format!("popout-later-{ix}")))
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .cursor_pointer()
                    .hover(|el| el.bg(accent.opacity(0.22)))
                    .on_click(move |_, _, cx| {
                        jump(channel.clone(), Some(ts.clone()), false, cx);
                    })
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(item.author.clone()),
                            )
                            .child(div().text_xs().text_color(muted).child(item.label.clone())),
                    )
                    .child(div().text_sm().child(item.text.clone()))
            }));
        }
        list
    }

    pub(super) fn render_canvas_body(
        &self,
        doc: &crate::canvas::CanvasDoc,
        file_id: &Option<String>,
        open_browser_url: &Option<String>,
        on_jump: &JumpHandler,
        cx: &App,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let open_browser = open_browser_url.clone().map(|url| {
            Rc::new(move |_: &mut Window, cx: &mut App| cx.open_url(&url))
                as crate::canvas::OpenBrowserFn
        });
        let on_edit = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |op, _: &mut Window, _: &mut App| {
                (send)(
                    CollabCommand::EditCanvas {
                        file_id: fid.clone(),
                        op,
                    }
                    .into(),
                );
            }) as crate::canvas::CanvasEditFn
        });
        let on_load_comments = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |_: &mut Window, _: &mut App| {
                (send)(
                    CollabCommand::LoadCanvasComments {
                        canvas_id: fid.clone(),
                    }
                    .into(),
                );
            }) as crate::canvas::CanvasCommentsLoadFn
        });
        let on_post_comment = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |text, thread_ts, _: &mut Window, _: &mut App| {
                (send)(
                    CollabCommand::PostCanvasComment {
                        canvas_id: fid.clone(),
                        text,
                        thread_ts,
                    }
                    .into(),
                );
            }) as crate::canvas::CanvasCommentPostFn
        });
        let on_resolve_comment = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |thread_ts, resolved, _: &mut Window, _: &mut App| {
                (send)(
                    CollabCommand::ResolveCanvasComment {
                        canvas_id: fid.clone(),
                        thread_ts,
                        resolved,
                    }
                    .into(),
                );
            }) as crate::canvas::CanvasCommentResolveFn
        });
        let on_resolve_all_comments = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |_: &mut Window, _: &mut App| {
                (send)(
                    CollabCommand::ResolveAllCanvasComments {
                        canvas_id: fid.clone(),
                    }
                    .into(),
                );
            }) as crate::canvas::CanvasCommentResolveAllFn
        });
        let on_set_access = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(
                move |req: crate::canvas::ShareAccessRequest, _: &mut Window, _: &mut App| {
                    (send)(
                        CollabCommand::SetCanvasAccess {
                            canvas_id: fid.clone(),
                            access_level: req.access_level,
                            channel_ids: req.channel_ids,
                            user_ids: req.user_ids,
                            post_permalink: req.post_permalink,
                        }
                        .into(),
                    );
                },
            ) as crate::canvas::CanvasAccessSetFn
        });
        let on_delete_access = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |channel_ids, user_ids, _: &mut Window, _: &mut App| {
                (send)(
                    CollabCommand::DeleteCanvasAccess {
                        canvas_id: fid.clone(),
                        channel_ids,
                        user_ids,
                    }
                    .into(),
                );
            }) as crate::canvas::CanvasAccessDeleteFn
        });
        let on_delete_canvas = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |_: &mut Window, _: &mut App| {
                (send)(
                    CollabCommand::DeleteCanvas {
                        canvas_id: fid.clone(),
                    }
                    .into(),
                );
            }) as crate::canvas::CanvasDeleteFn
        });
        let channel_lookup = crate::canvas::channel_lookup_from_fixtures(self.fixtures.clone());
        let jump_ch = on_jump.clone();
        let on_open_channel: crate::canvas::CanvasOpenChannelFn =
            Rc::new(move |channel_id, _window, cx| {
                jump_ch(channel_id, None, false, cx);
            });
        let on_print_canvas: crate::canvas::CanvasExportFn =
            Rc::new(
                move |doc, window, cx| match crate::canvas::trigger_canvas_print(&doc) {
                    Ok(_) => actions::notify(
                        "Print canvas",
                        "Opened printable canvas preview.",
                        window,
                        cx,
                    ),
                    Err(err) => actions::notify("Print canvas", err, window, cx),
                },
            );
        let on_save_pdf: crate::canvas::CanvasExportFn = Rc::new(move |doc, window, cx| {
            let stem = crate::canvas::canvas_export_stem(&doc.title);
            let target = std::env::temp_dir().join(format!("{stem}.pdf"));
            match crate::canvas::save_canvas_pdf_to_path(&doc, &target) {
                Ok(saved) => actions::notify(
                    "Saved PDF",
                    format!("Exported to {}", saved.display()),
                    window,
                    cx,
                ),
                Err(err) => actions::notify("Save as PDF", err, window, cx),
            }
        });
        let on_upload_file: crate::canvas::CanvasUploadFileFn = {
            let send = self.send.clone();
            let upload_channel = self.fixtures.messages.keys().next().cloned();
            let is_live = file_id.is_some();
            Rc::new(move |on_done, window: &mut Window, cx: &mut App| {
                let rx = cx.prompt_for_paths(gpui_kit::gpui::PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("Upload file to canvas".into()),
                });
                let send_u = send.clone();
                let ch_u = upload_channel.clone();
                window
                    .spawn(cx, async move |cx| {
                        let Ok(Ok(Some(paths))) = rx.await else {
                            return;
                        };
                        let Some(path) = paths.into_iter().next() else {
                            return;
                        };
                        let filename = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("attachment")
                            .to_owned();
                        let bytes = if is_live {
                            std::fs::read(&path).ok().filter(|b| !b.is_empty())
                        } else {
                            None
                        };
                        let url = format!("file://{}", path.display());
                        let _ = cx.update(|window, cx| {
                            if let (Some(ch), Some(bytes)) = (ch_u.clone(), bytes) {
                                (send_u)(
                                    CollabCommand::UploadFiles {
                                        channel: ch,
                                        thread_ts: None,
                                        content: None,
                                        files: vec![slackless_core::api::UploadFile {
                                            filename: filename.clone(),
                                            bytes,
                                        }],
                                    }
                                    .into(),
                                );
                            }
                            on_done(
                                crate::canvas::UploadedCanvasFile {
                                    filename: filename.clone(),
                                    url,
                                },
                                window,
                                cx,
                            );
                            actions::notify(
                                "Inserted file",
                                format!("Added {filename} to canvas."),
                                window,
                                cx,
                            );
                        });
                    })
                    .detach();
            })
        };
        let user_lookup = crate::canvas::user_lookup_from_fixtures(self.fixtures.clone());
        let on_open_profile: crate::canvas::CanvasOpenProfileFn = {
            let fixtures = self.fixtures.clone();
            let send = self.send.clone();
            let is_live = file_id.is_some();
            Rc::new(move |user_id, window: &mut Window, cx: &mut App| {
                if is_live {
                    (send)(WorkspaceCommand::FetchUserProfile(user_id.clone()).into());
                }
                if let Some(team) = fixtures.workspaces.first().filter(|t| !t.domain.is_empty()) {
                    cx.open_url(&format!(
                        "https://{}.slack.com/team/{}",
                        team.domain, user_id
                    ));
                } else {
                    let name = fixtures.user_name(&user_id).to_owned();
                    actions::notify("Profile", format!("User: {name}"), window, cx);
                }
            })
        };
        let on_open_embed: crate::canvas::CanvasOpenEmbedFn = {
            let fixtures = self.fixtures.clone();
            Rc::new(move |kind, fid, url, window: &mut Window, cx: &mut App| {
                if url.starts_with("https://") || url.starts_with("http://") {
                    cx.open_url(&url);
                } else if let Some(team) =
                    fixtures.workspaces.first().filter(|t| !t.domain.is_empty())
                    && !fid.is_empty()
                {
                    let seg = match kind {
                        crate::canvas::CanvasEmbedKind::Canvas => "docs",
                        crate::canvas::CanvasEmbedKind::List => "lists",
                    };
                    cx.open_url(&format!(
                        "https://{}.slack.com/{seg}/{}/{fid}",
                        team.domain, team.id
                    ));
                } else {
                    actions::notify(
                        "Embedded link",
                        format!("Cannot open embedded item {fid}"),
                        window,
                        cx,
                    );
                }
            })
        };
        let on_open_media: crate::canvas::CanvasOpenMediaFn =
            Rc::new(move |url, _kind, window: &mut Window, cx: &mut App| {
                if url.starts_with("https://") || url.starts_with("http://") {
                    cx.open_url(&url);
                } else {
                    actions::notify(
                        "Media link",
                        format!("Cannot open unsupported media URL: {url}"),
                        window,
                        cx,
                    );
                }
            });
        let key = file_id.as_deref().unwrap_or("popout-canvas");
        crate::canvas::render_popout_canvas(
            key,
            doc,
            "",
            None,
            crate::canvas::CanvasActions {
                open_browser,
                on_edit,
                on_upload_file: Some(on_upload_file),
                canvas_url: open_browser_url.clone(),
                on_open_media: Some(on_open_media),
                user_lookup: Some(user_lookup),
                channel_lookup: Some(channel_lookup),
                on_open_profile: Some(on_open_profile),
                on_open_channel: Some(on_open_channel),
                on_open_embed: Some(on_open_embed),
                on_load_comments,
                on_post_comment,
                on_resolve_comment,
                on_resolve_all_comments,
                on_set_access,
                on_delete_access,
                on_delete_canvas,
                on_print_canvas: Some(on_print_canvas),
                on_save_pdf: Some(on_save_pdf),
                ..Default::default()
            },
            file_id.is_none(),
            cx,
        )
    }

    pub(super) fn render_files_body(
        &self,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let theme = cx.theme();
        let (border, muted, accent) = (theme.border, theme.muted_foreground, theme.accent);
        let mut list = v_flex()
            .id("popout-files")
            .flex_1()
            .min_h_0()
            .px_4()
            .py_3()
            .gap_2()
            .overflow_y_scroll();
        if self.files_items.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .text_sm()
                    .text_color(muted)
                    .child("No files visible."),
            );
        } else {
            list = list.children(self.files_items.iter().enumerate().map(
                |(ix, (name, meta, url))| {
                    let url_opt = url.clone();
                    v_flex()
                        .id(SharedString::from(format!("popout-file-{ix}")))
                        .gap_0p5()
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .when_some(url_opt, |card, link| {
                            card.cursor_pointer()
                                .hover(|el| el.bg(accent.opacity(0.22)))
                                .on_click(move |_, _, cx| cx.open_url(&link))
                        })
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name.clone()),
                        )
                        .child(div().text_xs().text_color(muted).child(meta.clone()))
                },
            ));
        }
        list
    }
}
