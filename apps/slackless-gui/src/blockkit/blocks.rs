//! Block Kit container and modern block renderers (video, call, alert, card, plan, tables, etc.).

use super::*;

/// Slack's Block Kit `video` block: clickable title + thumbnail preview + description/provider.
pub(super) fn video_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, muted, fg, bg) = (
        theme.border,
        theme.muted_foreground,
        theme.foreground,
        theme.muted,
    );
    let title = text_of(&raw["title"])
        .or_else(|| raw["title"].as_str())
        .unwrap_or("Video")
        .to_owned();
    let url = raw["title_url"]
        .as_str()
        .or_else(|| raw["video_url"].as_str())
        .map(str::to_owned);
    let description = text_of(&raw["description"])
        .or_else(|| raw["description"].as_str())
        .map(|s| plain_label(s, cx));
    let provider = raw["provider_name"]
        .as_str()
        .or_else(|| raw["author_name"].as_str())
        .map(str::to_owned);
    let thumb_url = raw["thumbnail_url"].as_str().map(str::to_owned);

    let title_el: AnyElement = match url.clone() {
        Some(href) => routed_link(
            format!("bk-video-{}-{ix}", kcx.ts.as_str()),
            plain_label(&title, cx),
            href,
            kcx,
            FontWeight::SEMIBOLD,
            cx,
        )
        .into_any_element(),
        None => div()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(fg)
            .child(plain_label(&title, cx))
            .into_any_element(),
    };

    let mut card = v_flex()
        .w_full()
        .max_w(px(440.))
        .gap_1p5()
        .p_2p5()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .bg(bg);
    if let Some(prov) = provider {
        let icon_el: AnyElement = if let Some(icon_url) = raw["provider_icon_url"].as_str() {
            div()
                .size(px(14.))
                .flex_none()
                .overflow_hidden()
                .rounded(px(3.))
                .child(
                    crate::media::Picture::new(ImageSource::from(SharedString::from(
                        icon_url.to_owned(),
                    )))
                    .fit(ObjectFit::Cover)
                    .radius(px(3.)),
                )
                .into_any_element()
        } else {
            Icon::new(IconName::Video)
                .size(px(12.))
                .text_color(muted)
                .into_any_element()
        };
        card = card.child(
            h_flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(muted)
                .child(icon_el)
                .child(prov),
        );
    }
    card = card.child(title_el);
    if let Some(desc) = description {
        card = card.child(div().text_xs().text_color(muted).child(desc));
    }
    let direct_video_url = raw["video_url"].as_str().map(str::to_owned);
    if let Some(thumb) = thumb_url {
        let vid_id = format!("bk-video-{}-{ix}", kcx.ts.as_str());
        if let Some(snap) = crate::media::inline_video_snapshot(&vid_id, cx) {
            let fg = crate::theme::media_overlay_foreground();
            let id_toggle = vid_id.clone();
            let id_rate = vid_id.clone();
            let id_mute = vid_id.clone();
            let id_close = vid_id.clone();
            let id_play = vid_id.clone();
            let poster = Some(ImageSource::from(SharedString::from(thumb)));
            card = card.child(
                div()
                    .id(SharedString::from(format!(
                        "bk-video-active-{}-{ix}",
                        kcx.ts.as_str()
                    )))
                    .relative()
                    .w_full()
                    .h(px(220.))
                    .rounded_md()
                    .overflow_hidden()
                    .bg(theme.secondary)
                    .cursor_pointer()
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        crate::media::toggle_inline_video_pause(&id_toggle, window, cx);
                    })
                    .child(crate::media::render_video_surface(
                        snap.surface,
                        poster,
                        px(6.),
                    ))
                    // The dock follows this card's visibility, as it does the file card's.
                    .child(crate::media::inline_video_visibility_probe(vid_id.clone()))
                    .child(
                        h_flex()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .items_center()
                            .justify_between()
                            .px_2()
                            .py_1()
                            .bg(crate::theme::media_overlay_scrim_strong())
                            .on_mouse_down(gpui_kit::gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation()
                            })
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "bk-video-play-{}-{ix}",
                                                kcx.ts.as_str()
                                            )))
                                            .cursor_pointer()
                                            .on_click(move |_, window, cx| {
                                                cx.stop_propagation();
                                                crate::media::toggle_inline_video_pause(
                                                    &id_play, window, cx,
                                                );
                                            })
                                            .child(
                                                Icon::new(if snap.playing {
                                                    IconName::Pause
                                                } else {
                                                    IconName::Play
                                                })
                                                .size(px(14.))
                                                .text_color(fg),
                                            ),
                                    )
                                    .child(div().text_size(px(10.)).text_color(fg).child(format!(
                                        "{} / {}",
                                        crate::media::format_video_time(snap.position_secs),
                                        crate::media::format_video_time(snap.duration_secs)
                                    ))),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "bk-video-rate-{}-{ix}",
                                                kcx.ts.as_str()
                                            )))
                                            .px_1()
                                            .py(px(1.))
                                            .rounded_sm()
                                            .text_size(px(10.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(fg)
                                            .cursor_pointer()
                                            .on_click(move |_, window, cx| {
                                                cx.stop_propagation();
                                                crate::media::cycle_inline_video_rate(
                                                    &id_rate, window, cx,
                                                );
                                            })
                                            .child(crate::media::format_playback_rate(snap.rate)),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "bk-video-mute-{}-{ix}",
                                                kcx.ts.as_str()
                                            )))
                                            .p_1()
                                            .cursor_pointer()
                                            .on_click(move |_, window, cx| {
                                                cx.stop_propagation();
                                                crate::media::toggle_inline_video_mute(
                                                    &id_mute, window, cx,
                                                );
                                            })
                                            .child(
                                                Icon::new(if snap.muted {
                                                    IconName::VolumeX
                                                } else {
                                                    IconName::Volume2
                                                })
                                                .size(px(14.))
                                                .text_color(fg),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "bk-video-close-{}-{ix}",
                                                kcx.ts.as_str()
                                            )))
                                            .p_1()
                                            .cursor_pointer()
                                            .on_click(move |_, window, cx| {
                                                cx.stop_propagation();
                                                crate::media::stop_inline_video(
                                                    &id_close, window, cx,
                                                );
                                            })
                                            .child(
                                                Icon::new(IconName::X).size(px(14.)).text_color(fg),
                                            ),
                                    ),
                            ),
                    ),
            );
        } else {
            let thumb_box = div()
                .relative()
                .w_full()
                .h(px(180.))
                .rounded_md()
                .overflow_hidden()
                .bg(theme.secondary)
                .child(
                    crate::media::Picture::new(ImageSource::from(SharedString::from(thumb)))
                        .fit(ObjectFit::Contain)
                        .radius(px(6.)),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(40.))
                                .rounded_full()
                                .bg(crate::theme::media_overlay_scrim_strong())
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Play)
                                        .size(px(18.))
                                        .text_color(crate::theme::media_overlay_foreground()),
                                ),
                        ),
                );
            if let Some(href) = direct_video_url.clone().or(url) {
                let (is_direct_media, media_ext) = {
                    let path_part = href
                        .split(['?', '#'])
                        .next()
                        .unwrap_or(&href)
                        .to_ascii_lowercase();
                    let ext = if path_part.ends_with(".mov") {
                        "mov"
                    } else if path_part.ends_with(".webm") {
                        "webm"
                    } else {
                        "mp4"
                    };
                    let direct = path_part.ends_with(".mp4")
                        || path_part.ends_with(".mov")
                        || path_part.ends_with(".webm")
                        || crate::media::is_hls_url(&href)
                        || href.to_ascii_lowercase().contains("files.slack.com");
                    (direct, ext.to_owned())
                };
                let target = LinkTarget::Url(href.clone().into());
                let links = kcx.links.clone();
                card = card.child(
                    div()
                        .id(SharedString::from(format!(
                            "bk-video-thumb-{}-{ix}",
                            kcx.ts.as_str()
                        )))
                        .cursor_pointer()
                        .on_click(move |_, window, cx| {
                            if is_direct_media {
                                crate::media::play_inline_video(
                                    vid_id.clone(),
                                    href.clone(),
                                    media_ext.clone(),
                                    None,
                                    window,
                                    cx,
                                );
                            } else {
                                messages::route_link(links.as_ref(), target.clone(), window, cx);
                            }
                        })
                        .child(thumb_box),
                );
            } else {
                card = card.child(thumb_box);
            }
        }
    }
    card
}

/// Slack's Block Kit `call` block (third-party / legacy calls, `F-COLLAB-086..088`):
/// supports both inline `call` / `call.v1` JSON and hydrated `calls.info` updates with provider
/// badges, live participant list, Join / Open in browser / Copy link / Refresh actions, and
/// ended duration state.
pub(super) fn call_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let call_outer = raw.get("call").unwrap_or(raw);
    let call = call_outer
        .get("v1")
        .filter(|v| v.is_object())
        .unwrap_or(call_outer);
    let call_id = call["id"]
        .as_str()
        .or_else(|| raw["call_id"].as_str())
        .or_else(|| call_outer["id"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let hydrated = call_id
        .as_deref()
        .and_then(|id| kcx.kit.and_then(|kit| kit.read(cx).call_info(id).cloned()));

    let title = hydrated
        .as_ref()
        .and_then(|h| h.title.as_deref())
        .filter(|s| !s.is_empty())
        .or_else(|| call["title"].as_str().filter(|s| !s.is_empty()))
        .or_else(|| call["name"].as_str().filter(|s| !s.is_empty()))
        .unwrap_or("Call")
        .to_owned();
    let date_start = hydrated
        .as_ref()
        .and_then(|h| h.date_start)
        .or_else(|| call["date_start"].as_i64());
    let date_end = hydrated
        .as_ref()
        .and_then(|h| h.date_end)
        .or_else(|| call["date_end"].as_i64());
    let ended = date_end.is_some_and(|t| t > 0) || call["has_ended"].as_bool().unwrap_or(false);

    let participant_names: Vec<String> = if let Some(ref h) = hydrated {
        h.users
            .iter()
            .filter_map(|p| {
                p.slack_id
                    .as_deref()
                    .map(|uid| kcx.fixtures.user_name(&UserId::from(uid)).to_owned())
                    .or_else(|| p.display_name.clone())
            })
            .collect()
    } else {
        call["active_participants"]
            .as_array()
            .or_else(|| call["users"].as_array())
            .or_else(|| call["all_participants"].as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| {
                        p["slack_id"]
                            .as_str()
                            .map(|uid| kcx.fixtures.user_name(&UserId::from(uid)).to_owned())
                            .or_else(|| p["display_name"].as_str().map(str::to_owned))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let participants = participant_names.len();

    let web_join_url = hydrated
        .as_ref()
        .and_then(|h| h.join_url.clone())
        .or_else(|| call["join_url"].as_str().map(str::to_owned))
        .filter(|s| !s.is_empty());
    let desktop_join_url = hydrated
        .as_ref()
        .and_then(|h| h.desktop_app_join_url.clone())
        .or_else(|| call["desktop_app_join_url"].as_str().map(str::to_owned))
        .filter(|s| !s.is_empty());
    let primary_join_url = desktop_join_url.clone().or_else(|| web_join_url.clone());
    let copy_url = web_join_url.clone().or_else(|| desktop_join_url.clone());
    let provider = web_join_url
        .as_deref()
        .and_then(slackless_core::domain::calling::call_provider_for_url)
        .or_else(|| {
            desktop_join_url
                .as_deref()
                .and_then(slackless_core::domain::calling::call_provider_for_url)
        });

    let duration_label = match (date_start, date_end) {
        (Some(start), Some(end)) if end > start => {
            let mins = ((end - start) + 30) / 60;
            if mins >= 60 {
                Some(format!("{}h {}m", mins / 60, mins % 60))
            } else if mins >= 1 {
                Some(format!("{mins}m"))
            } else {
                Some("<1m".to_owned())
            }
        }
        _ => None,
    };

    let status = if ended {
        match (duration_label, participants) {
            (Some(dur), n) if n > 0 => format!(
                "Call ended · {dur} · {n} {}",
                if n == 1 {
                    "participant"
                } else {
                    "participants"
                }
            ),
            (Some(dur), _) => format!("Call ended · {dur}"),
            (None, n) if n > 0 => format!(
                "Call ended · {n} {}",
                if n == 1 {
                    "participant"
                } else {
                    "participants"
                }
            ),
            (None, _) => "Call ended".to_owned(),
        }
    } else if participants > 0 {
        let preview = if participants <= 3 {
            participant_names.join(", ")
        } else {
            format!(
                "{}, {} +{} more",
                participant_names[0],
                participant_names[1],
                participants - 2
            )
        };
        format!("Live · {preview}")
    } else {
        "Live call".to_owned()
    };

    let mut action_row = h_flex().flex_none().items_center().gap_1p5();
    if !ended && let Some(href) = primary_join_url {
        let target = LinkTarget::Url(href.into());
        let links = kcx.links.clone();
        action_row = action_row.child(
            Button::new(SharedString::from(format!(
                "bk-call-join-{}-{ix}",
                kcx.ts.as_str()
            )))
            .primary()
            .with_size(Size::XSmall)
            .icon(IconName::Video)
            .label("Join")
            .on_click(move |_, window, cx| {
                messages::route_link(links.as_ref(), target.clone(), window, cx)
            }),
        );
    }
    if !ended
        && let (Some(desktop), Some(web)) = (&desktop_join_url, &web_join_url)
        && desktop != web
    {
        let target = LinkTarget::Url(web.clone().into());
        let links = kcx.links.clone();
        action_row = action_row.child(
            Button::new(SharedString::from(format!(
                "bk-call-web-{}-{ix}",
                kcx.ts.as_str()
            )))
            .outline()
            .with_size(Size::XSmall)
            .icon(IconName::ExternalLink)
            .tooltip("Join in browser")
            .on_click(move |_, window, cx| {
                messages::route_link(links.as_ref(), target.clone(), window, cx)
            }),
        );
    }
    if let Some(url) = copy_url {
        action_row = action_row.child(
            Button::new(SharedString::from(format!(
                "bk-call-copy-{}-{ix}",
                kcx.ts.as_str()
            )))
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::Link)
            .tooltip("Copy call link")
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(url.clone()));
                actions::notify(
                    "Call link copied",
                    "Copied meeting URL to clipboard.",
                    window,
                    cx,
                );
            }),
        );
    }
    if !ended && let (Some(id), Some(on_refresh)) = (call_id.clone(), kcx.on_call_refresh.clone()) {
        action_row = action_row.child(
            Button::new(SharedString::from(format!(
                "bk-call-refresh-{}-{ix}",
                kcx.ts.as_str()
            )))
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::RefreshCw)
            .tooltip("Refresh call status")
            .on_click(move |_, _, cx| on_refresh(id.clone(), cx)),
        );
    }

    h_flex()
        .w_full()
        .max_w(px(500.))
        .items_center()
        .gap_3()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(if ended {
            theme.border
        } else {
            theme.success.opacity(0.45)
        })
        .bg(theme.muted)
        .child(
            div()
                .flex_none()
                .size(px(36.))
                .rounded_md()
                .bg(if ended {
                    theme.secondary
                } else {
                    theme.success.opacity(0.18)
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(if ended {
                        IconName::PhoneOff
                    } else if provider.is_some() {
                        IconName::Video
                    } else {
                        IconName::Headphones
                    })
                    .size(px(16.))
                    .text_color(if ended {
                        theme.muted_foreground
                    } else {
                        theme.success
                    }),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(plain_label(&title, cx)),
                        )
                        .when_some(provider, |hdr, prov| {
                            hdr.child(
                                div()
                                    .flex_none()
                                    .px_1p5()
                                    .py(px(1.))
                                    .rounded_sm()
                                    .bg(theme.secondary)
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.muted_foreground)
                                    .child(prov),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .when(!ended, |r| {
                            r.child(div().size(px(6.)).rounded_full().bg(theme.success))
                        })
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(status),
                        ),
                ),
        )
        .child(action_row)
}

/// Modern Block Kit `alert` / `callout` block (`F-COLLAB-713`): tinted left-accent banner.
pub(super) fn alert_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let accent = match raw["style"]
        .as_str()
        .or_else(|| raw["severity"].as_str())
        .or_else(|| raw["variant"].as_str())
        .unwrap_or("info")
    {
        "danger" | "error" | "critical" => theme.danger,
        "warning" | "warn" => theme.warning,
        "success" | "positive" => theme.success,
        _ => theme.link,
    };
    let mut body = v_flex().flex_1().min_w_0().gap_1();
    if let Some(title) = text_of(&raw["title"]).or_else(|| raw["title"].as_str()) {
        body = body.child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(plain_label(title, cx)),
        );
    }
    if let Some(el) = text_object(&raw["text"], kcx, false, cx)
        .or_else(|| text_object(&raw["body"], kcx, false, cx))
        .or_else(|| {
            raw["message"]
                .as_str()
                .or_else(|| raw["text"].as_str())
                .map(|s| messages::mrkdwn(s, kcx, false, cx).into_any_element())
        })
    {
        body = body.child(el);
    }
    if let Some(nested) = raw["blocks"].as_array() {
        for (n, child) in nested.iter().enumerate() {
            let kind = child["type"].as_str().unwrap_or_default();
            if let Some(el) = block(kind, child, ix * 31 + n, kcx, cx) {
                body = body.child(el);
            }
        }
    }
    h_flex()
        .w_full()
        .max_w(px(640.))
        .items_stretch()
        .gap_2p5()
        .p_2p5()
        .rounded_md()
        .border_1()
        .border_color(accent.opacity(0.35))
        .bg(accent.opacity(0.08))
        .child(div().flex_none().w(px(3.)).rounded_full().bg(accent))
        .child(body)
}

/// Modern Block Kit `card` / `container` block (`F-COLLAB-714`, `F-COLLAB-716`): bordered
/// structured card with optional title, subtitle, fields, nested blocks, and actions.
pub(super) fn card_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let mut card = v_flex()
        .w_full()
        .max_w(px(560.))
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted);
    if let Some(title) = text_of(&raw["title"]).or_else(|| raw["title"].as_str()) {
        let slack_icon: Option<AnyElement> = raw["icon_url"]
            .as_str()
            .or_else(|| raw["slack_icon"]["url"].as_str())
            .map(|u| {
                div()
                    .size(px(18.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded(px(4.))
                    .child(
                        crate::media::Picture::new(ImageSource::from(SharedString::from(
                            u.to_owned(),
                        )))
                        .fit(ObjectFit::Cover)
                        .radius(px(4.)),
                    )
                    .into_any_element()
            })
            .or_else(|| {
                raw["slack_icon"]
                    .as_str()
                    .or_else(|| raw["icon"].as_str())
                    .map(|name| {
                        let icon = match name {
                            "canvas" | "document" => IconName::FileText,
                            "list" | "checklist" => IconName::ListChecks,
                            "workflow" | "lightning" => IconName::Zap,
                            "calendar" => IconName::Calendar,
                            "warning" | "alert" => IconName::TriangleAlert,
                            _ => IconName::Sparkles,
                        };
                        Icon::new(icon)
                            .size(px(15.))
                            .text_color(theme.link)
                            .into_any_element()
                    })
            });
        let title_el: AnyElement = match raw["title_url"].as_str().or_else(|| raw["url"].as_str()) {
            Some(href) => routed_link(
                format!("bk-card-title-{}-{ix}", kcx.ts.as_str()),
                plain_label(title, cx),
                href.to_owned(),
                kcx,
                FontWeight::SEMIBOLD,
                cx,
            )
            .into_any_element(),
            None => div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(plain_label(title, cx))
                .into_any_element(),
        };
        card = card.child(
            h_flex()
                .items_center()
                .gap_2()
                .children(slack_icon)
                .child(title_el),
        );
    }
    if let Some(sub) = text_of(&raw["subtitle"]).or_else(|| raw["subtitle"].as_str()) {
        card = card.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(plain_label(sub, cx)),
        );
    }
    if let Some(el) = text_object(&raw["body"], kcx, false, cx)
        .or_else(|| text_object(&raw["text"], kcx, false, cx))
    {
        card = card.child(el);
    }
    if let Some(fields) = raw["fields"].as_array() {
        for pair in fields.chunks(2) {
            card = card.child(
                h_flex()
                    .items_start()
                    .gap_4()
                    .children(pair.iter().map(|field| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .children(text_object(field, kcx, false, cx))
                    })),
            );
        }
    }
    for key in ["blocks", "elements"] {
        if let Some(children) = raw[key].as_array() {
            for (n, child) in children.iter().enumerate() {
                let kind = child["type"].as_str().unwrap_or_default();
                if let Some(el) = block(kind, child, ix * 37 + n, kcx, cx) {
                    card = card.child(el);
                }
            }
        }
    }
    if let Some(actions) = raw["actions"].as_array() {
        card = card.child(h_flex().flex_wrap().items_center().gap_2().children(
            actions.iter().enumerate().map(|(n, element)| {
                let key = control_key(kcx.ts, raw, ix, element, n);
                block_element(element, raw, key, kcx, cx)
            }),
        ));
    }
    card
}

/// Modern Block Kit `context_actions` block (`F-COLLAB-717`): compact strip of inline
/// feedback buttons (`positive_button` / `negative_button`), icon buttons, or action elements.
pub(super) fn context_actions_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let mut row = h_flex().flex_wrap().items_center().gap_1p5();
    if let Some(elements) = raw["elements"].as_array() {
        for (n, element) in elements.iter().enumerate() {
            match element["type"].as_str().unwrap_or_default() {
                "feedback_buttons" => {
                    for (sub_ix, field) in ["positive_button", "negative_button"].iter().enumerate()
                    {
                        if let Some(btn) = element.get(*field).filter(|v| v.is_object()) {
                            let mut synthetic = btn.clone();
                            synthetic["type"] = "button".into();
                            if synthetic["action_id"].is_null() {
                                synthetic["action_id"] = element["action_id"].clone();
                            }
                            let key = format!(
                                "{}-fb{sub_ix}",
                                control_key(kcx.ts, raw, ix, &synthetic, n)
                            );
                            row = row.child(block_element(&synthetic, raw, key, kcx, cx));
                        }
                    }
                }
                "icon_button" => {
                    let key = control_key(kcx.ts, raw, ix, element, n);
                    row = row.child(block_element(element, raw, key, kcx, cx));
                }
                _ => {
                    let key = control_key(kcx.ts, raw, ix, element, n);
                    row = row.child(block_element(element, raw, key, kcx, cx));
                }
            }
        }
    }
    row
}

/// Modern Block Kit `task_card` block (`F-COLLAB-732`): structured status card for agent and
/// workflow steps with status badge, title, optional details/output, sources, and actions.
pub(super) fn task_card_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let status = raw["status"].as_str().unwrap_or("pending");
    let (status_icon, status_label, status_color) = match status {
        "complete" | "completed" | "done" | "success" => {
            (IconName::Check, "Complete", theme.success)
        }
        "in_progress" | "running" => (IconName::Clock, "In progress", theme.link),
        "error" | "failed" => (IconName::X, "Error", theme.danger),
        _ => (IconName::CircleDot, "Pending", theme.muted_foreground),
    };
    let title = text_of(&raw["title"])
        .or_else(|| raw["title"].as_str())
        .unwrap_or("Task");

    let mut card = v_flex()
        .w_full()
        .max_w(px(580.))
        .gap_1p5()
        .p_2p5()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .size(px(20.))
                                .rounded_full()
                                .bg(status_color.opacity(0.16))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(status_icon)
                                        .size(px(12.))
                                        .text_color(status_color),
                                ),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(plain_label(title, cx)),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .px_1p5()
                        .py(px(1.))
                        .rounded_sm()
                        .bg(status_color.opacity(0.14))
                        .text_xs()
                        .text_color(status_color)
                        .child(status_label),
                ),
        );

    for key in ["details", "output"] {
        let field = &raw[key];
        if field.is_null() {
            continue;
        }
        if field["type"].as_str() == Some("rich_text")
            && let Ok(parsed) = serde_json::from_value::<Block>(field.clone())
        {
            card = card.child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .pl(px(28.))
                    .children(messages::render_block(&parsed, 0, kcx, false, cx)),
            );
            continue;
        }
        if let Some(el) = text_object(field, kcx, false, cx).or_else(|| {
            field
                .as_str()
                .map(|s| messages::mrkdwn(s, kcx, false, cx).into_any_element())
        }) {
            card = card.child(div().pl(px(28.)).child(el));
        }
    }

    if let Some(sources) = raw["sources"].as_array().filter(|s| !s.is_empty()) {
        let mut src_row = h_flex().flex_wrap().items_center().gap_2().pl(px(28.));
        for (n, src) in sources.iter().enumerate() {
            if let Some(url) = src["url"].as_str() {
                let label = src["text"]
                    .as_str()
                    .or_else(|| text_of(&src["text"]))
                    .unwrap_or(url);
                src_row = src_row.child(routed_link(
                    format!("bk-task-src-{}-{ix}-{n}", kcx.ts.as_str()),
                    plain_label(label, cx),
                    url.to_owned(),
                    kcx,
                    FontWeight::NORMAL,
                    cx,
                ));
            }
        }
        card = card.child(src_row);
    }

    if let Some(actions) = raw["actions"].as_array().filter(|a| !a.is_empty()) {
        card = card.child(
            h_flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .pl(px(28.))
                .children(actions.iter().enumerate().map(|(n, element)| {
                    let key = control_key(kcx.ts, raw, ix, element, n);
                    block_element(element, raw, key, kcx, cx)
                })),
        );
    }

    card
}

/// Modern Block Kit `plan` block (`F-COLLAB-728`): titled group of `task_card` steps with
/// completion progress summary.
pub(super) fn plan_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let title = text_of(&raw["title"])
        .or_else(|| raw["title"].as_str())
        .unwrap_or("Plan");
    let tasks = raw["tasks"]
        .as_array()
        .or_else(|| raw["blocks"].as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let completed = tasks
        .iter()
        .filter(|t| {
            matches!(
                t["status"].as_str().unwrap_or_default(),
                "complete" | "completed" | "done" | "success"
            )
        })
        .count();

    let mut container = v_flex()
        .w_full()
        .max_w(px(600.))
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::ListChecks)
                                .size(px(15.))
                                .text_color(theme.link),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(plain_label(title, cx)),
                        ),
                )
                .when(!tasks.is_empty(), |hdr| {
                    hdr.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{completed} of {} complete", tasks.len())),
                    )
                }),
        );
    for (n, task) in tasks.iter().enumerate() {
        container = container.child(task_card_block(task, ix * 43 + n, kcx, cx));
    }
    container
}

/// Modern Block Kit `carousel` block (`F-COLLAB-715`): horizontal scrollable strip of cards.
pub(super) fn carousel_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let items = raw["elements"]
        .as_array()
        .or_else(|| raw["cards"].as_array())
        .or_else(|| raw["blocks"].as_array());
    let mut strip = h_flex().gap_3().items_stretch();
    if let Some(items) = items {
        for (n, item) in items.iter().enumerate() {
            let kind = item["type"].as_str().unwrap_or("card");
            if let Some(el) = block(kind, item, ix * 41 + n, kcx, cx) {
                strip = strip.child(div().w(px(280.)).flex_none().child(el));
            }
        }
    }
    div().w_full().min_w_0().child(
        div()
            .id(SharedString::from(format!(
                "bk-carousel-{}-{ix}",
                kcx.ts.as_str()
            )))
            .w_full()
            .overflow_x_scroll()
            .pb_1()
            .child(strip),
    )
}

/// Slack's Block Kit `file` block/element (`F-COLLAB-750`): compact card for an embedded remote file.
pub(super) fn file_block(raw: &Json, cx: &App) -> Div {
    let theme = cx.theme();
    let label = raw["title"]
        .as_str()
        .or_else(|| text_of(&raw["title"]))
        .or_else(|| raw["name"].as_str())
        .or_else(|| raw["external_id"].as_str())
        .or_else(|| raw["file_id"].as_str())
        .unwrap_or("Attached file");
    let kind = raw["filetype"]
        .as_str()
        .or_else(|| raw["mimetype"].as_str())
        .map(str::to_uppercase);
    h_flex()
        .items_center()
        .gap_2()
        .px_2p5()
        .py_1p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .text_xs()
        .text_color(theme.foreground)
        .child(
            Icon::new(IconName::File)
                .size(px(14.))
                .text_color(theme.muted_foreground),
        )
        .child(plain_label(label, cx))
        .when_some(kind, |row, k| {
            row.child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(format!("· {k}")),
            )
        })
}

/// Block Kit `data_visualization` / `chart` block (`F-COLLAB-720`): compact horizontal bar chart
/// & summary table for bot/app numeric series (`data`, `points`, or `series`).
pub(super) fn data_visualization_block(raw: &Json, cx: &App) -> Div {
    let theme = cx.theme();
    let title = text_of(&raw["title"])
        .or_else(|| raw["title"].as_str())
        .unwrap_or("Chart");
    let subtitle = text_of(&raw["subtitle"]).or_else(|| raw["subtitle"].as_str());
    let mut rows: Vec<(String, f64)> = Vec::new();
    let source = raw["data"]
        .as_array()
        .or_else(|| raw["points"].as_array())
        .or_else(|| {
            raw["series"]
                .as_array()
                .and_then(|s| s.first())
                .and_then(|first| {
                    first["data"]
                        .as_array()
                        .or_else(|| first["points"].as_array())
                })
        });
    if let Some(items) = source {
        for (idx, item) in items.iter().take(12).enumerate() {
            let label = item["label"]
                .as_str()
                .or_else(|| item["name"].as_str())
                .or_else(|| item["x"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Item {}", idx + 1));
            let val = item["value"]
                .as_f64()
                .or_else(|| item["y"].as_f64())
                .or_else(|| item.as_f64())
                .unwrap_or(0.0);
            rows.push((label, val));
        }
    }
    let max_val = rows.iter().map(|(_, v)| v.abs()).fold(1.0f64, f64::max);

    let mut card = v_flex()
        .w_full()
        .max_w(px(520.))
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Activity)
                        .size(px(14.))
                        .text_color(theme.link),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(plain_label(title, cx)),
                ),
        )
        .when_some(subtitle, |col, sub| {
            col.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(plain_label(sub, cx)),
            )
        });
    if rows.is_empty() {
        card = card.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("No data points provided."),
        );
    } else {
        for (label, val) in rows {
            let pct = ((val.abs() / max_val) * 180.0).clamp(6.0, 180.0) as f32;
            let formatted = if (val.fract()).abs() < 1e-6 {
                format!("{}", val as i64)
            } else {
                format!("{val:.2}")
            };
            card = card.child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .w(px(110.))
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(plain_label(&label, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(10.))
                            .rounded_full()
                            .bg(theme.secondary)
                            .overflow_hidden()
                            .child(div().h_full().w(px(pct)).rounded_full().bg(theme.primary)),
                    )
                    .child(
                        div()
                            .w(px(54.))
                            .text_right()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(formatted),
                    ),
            );
        }
    }
    card
}

/// Block Kit `layout` / `columns` block (`F-COLLAB-726`): renders multi-column child blocks side by side.
pub(super) fn layout_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let columns = raw["columns"]
        .as_array()
        .or_else(|| raw["elements"].as_array())
        .or_else(|| raw["blocks"].as_array());
    let mut row = h_flex().w_full().min_w_0().items_start().gap_3();
    if let Some(cols) = columns {
        for (c_ix, col) in cols.iter().enumerate() {
            if let Some(nested) = col["blocks"]
                .as_array()
                .or_else(|| col["elements"].as_array())
            {
                let mut col_box = v_flex().flex_1().min_w_0().gap_1p5();
                for (b_ix, child) in nested.iter().enumerate() {
                    let kind = child["type"].as_str().unwrap_or_default();
                    if let Some(el) = block(kind, child, ix * 53 + c_ix * 11 + b_ix, kcx, cx) {
                        col_box = col_box.child(el);
                    }
                }
                row = row.child(col_box);
            } else {
                let kind = col["type"].as_str().unwrap_or("section");
                if let Some(el) = block(kind, col, ix * 53 + c_ix, kcx, cx) {
                    row = row.child(div().flex_1().min_w_0().child(el));
                }
            }
        }
    }
    row
}

/// Block Kit `canvas` / `canvas_unfurl` element (`F-COLLAB-737`, `F-COLLAB-738`).
pub(super) fn canvas_element(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let title = text_of(&raw["title"])
        .or_else(|| raw["title"].as_str())
        .or_else(|| raw["canvas_title"].as_str())
        .unwrap_or("Canvas");
    let preview = text_of(&raw["preview"])
        .or_else(|| raw["preview"].as_str())
        .or_else(|| raw["excerpt"].as_str())
        .or_else(|| text_of(&raw["text"]));
    let url = raw["url"]
        .as_str()
        .or_else(|| raw["permalink"].as_str())
        .map(str::to_owned);
    let title_el: AnyElement = match url {
        Some(href) => routed_link(
            format!("bk-canvas-{}-{ix}", kcx.ts.as_str()),
            plain_label(title, cx),
            href,
            kcx,
            FontWeight::SEMIBOLD,
            cx,
        )
        .into_any_element(),
        None => div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.foreground)
            .child(plain_label(title, cx))
            .into_any_element(),
    };
    v_flex()
        .w_full()
        .max_w(px(480.))
        .gap_1()
        .p_2p5()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::FileText)
                        .size(px(14.))
                        .text_color(theme.link),
                )
                .child(title_el)
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Canvas"),
                ),
        )
        .when_some(preview, |col, p| {
            col.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(plain_label(p, cx)),
            )
        })
}

/// Block Kit `list_record` element (`F-COLLAB-755`): structured Slack List item card.
pub(super) fn list_record_element(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let title = text_of(&raw["title"])
        .or_else(|| raw["title"].as_str())
        .or_else(|| raw["record_title"].as_str())
        .or_else(|| raw["name"].as_str())
        .unwrap_or("List item");
    let list_name = raw["list_title"]
        .as_str()
        .or_else(|| raw["list_name"].as_str());
    let status = raw["status"].as_str();
    let url = raw["url"]
        .as_str()
        .or_else(|| raw["permalink"].as_str())
        .map(str::to_owned);
    let title_el: AnyElement = match url {
        Some(href) => routed_link(
            format!("bk-listrec-{}-{ix}", kcx.ts.as_str()),
            plain_label(title, cx),
            href,
            kcx,
            FontWeight::SEMIBOLD,
            cx,
        )
        .into_any_element(),
        None => div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.foreground)
            .child(plain_label(title, cx))
            .into_any_element(),
    };
    h_flex()
        .w_full()
        .max_w(px(480.))
        .items_center()
        .justify_between()
        .gap_2()
        .p_2p5()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            h_flex()
                .min_w_0()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::ListChecks)
                        .size(px(14.))
                        .text_color(theme.link),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .child(title_el)
                        .when_some(list_name, |col, ln| {
                            col.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(plain_label(ln, cx)),
                            )
                        }),
                ),
        )
        .when_some(status, |row, st| {
            row.child(
                div()
                    .flex_none()
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .bg(theme.secondary)
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(plain_label(st, cx)),
            )
        })
}

/// Parse `#RGB` or `#RRGGBB` hex string into GPUI `Hsla` for Block Kit `color` elements (`F-COLLAB-743`).
pub(super) fn parse_hex_color(raw: &str) -> Option<Hsla> {
    let hex = raw.trim().trim_start_matches('#');
    if hex.len() == 3 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        let mut expanded = [0u8; 6];
        for (i, b) in hex.bytes().enumerate() {
            expanded[i * 2] = b;
            expanded[i * 2 + 1] = b;
        }
        return std::str::from_utf8(&expanded)
            .ok()
            .and_then(crate::messages::hex_color);
    }
    crate::messages::hex_color(hex)
}

/// Block Kit `color` element (`F-COLLAB-743`): inline swatch pill with parsed color dot + hex label.
pub(super) fn color_element(raw: &Json, cx: &App) -> Div {
    let theme = cx.theme();
    let hex = raw["value"]
        .as_str()
        .or_else(|| raw["color"].as_str())
        .unwrap_or("#888888");
    let swatch = parse_hex_color(hex).unwrap_or(theme.primary);
    h_flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded_full()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .child(
            div()
                .size(px(10.))
                .rounded_full()
                .bg(swatch)
                .border_1()
                .border_color(theme.foreground.opacity(0.2)),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.foreground)
                .child(hex.to_owned()),
        )
}

/// Block Kit `work_object` mention link (`F-COLLAB-772`).
pub(super) fn work_object_element(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let label = raw["title"]
        .as_str()
        .or_else(|| text_of(&raw["title"]))
        .or_else(|| raw["entity_type"].as_str())
        .unwrap_or("Work object");
    let url = raw["url"]
        .as_str()
        .or_else(|| raw["external_ref"]["url"].as_str());
    let inner: AnyElement = match url {
        Some(href) => routed_link(
            format!("bk-wo-{}-{ix}", kcx.ts.as_str()),
            plain_label(label, cx),
            href.to_owned(),
            kcx,
            FontWeight::MEDIUM,
            cx,
        )
        .into_any_element(),
        None => div()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.foreground)
            .child(plain_label(label, cx))
            .into_any_element(),
    };
    h_flex()
        .items_center()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .child(
            Icon::new(IconName::Briefcase)
                .size(px(12.))
                .text_color(theme.link),
        )
        .child(inner)
        .into_any_element()
}

/// Block Kit `workflow` mention link (`F-COLLAB-409`, `F-COLLAB-432`, `F-COLLAB-773`):
/// shows workflow title link plus compact details & copy-URL triggers when a trigger URL/ID is present.
pub(super) fn workflow_link_element(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let label = raw["title"]
        .as_str()
        .or_else(|| text_of(&raw["title"]))
        .or_else(|| raw["name"].as_str())
        .unwrap_or("Workflow");
    let url = raw["trigger"]["url"]
        .as_str()
        .or_else(|| raw["url"].as_str())
        .map(str::to_owned);
    let trigger_id = raw["trigger"]["id"]
        .as_str()
        .or_else(|| raw["trigger_id"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            url.as_deref()
                .and_then(slackless_core::api::extract_workflow_trigger_id)
        });
    let inner: AnyElement = match url.as_deref() {
        Some(href) => routed_link(
            format!("bk-wf-{}-{ix}", kcx.ts.as_str()),
            plain_label(label, cx),
            href.to_owned(),
            kcx,
            FontWeight::MEDIUM,
            cx,
        )
        .into_any_element(),
        None => div()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.foreground)
            .child(plain_label(label, cx))
            .into_any_element(),
    };
    let on_details = kcx.on_workflow_details.clone();
    let details_req = (trigger_id.is_some() || url.is_some()).then(|| WorkflowDetailsRequest {
        trigger_id,
        url: url.clone(),
        title: plain_label(label, cx),
    });
    h_flex()
        .items_center()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .child(
            Icon::new(IconName::Zap)
                .size(px(12.))
                .text_color(theme.link),
        )
        .child(inner)
        .when_some(on_details.zip(details_req), |row, (handler, req)| {
            row.child(
                Button::new(SharedString::from(format!(
                    "bk-wf-info-{}-{ix}",
                    kcx.ts.as_str()
                )))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::Info)
                .tooltip("View workflow details")
                .on_click(move |_, window, cx| handler(req.clone(), window, cx)),
            )
        })
        .when_some(url, |row, href| {
            row.child(
                Button::new(SharedString::from(format!(
                    "bk-wf-copy-{}-{ix}",
                    kcx.ts.as_str()
                )))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::Link)
                .tooltip("Copy workflow URL")
                .on_click(move |_, window, cx| {
                    cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(href.clone()));
                    actions::notify(
                        "Workflow link copied",
                        "Copied workflow URL to clipboard.",
                        window,
                        cx,
                    );
                }),
            )
        })
        .into_any_element()
}

/// Block Kit `url_source` attribution chip (`F-COLLAB-792`).
pub(super) fn url_source_element(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let url = raw["url"].as_str().unwrap_or_default();
    let label = raw["text"]
        .as_str()
        .or_else(|| text_of(&raw["text"]))
        .or_else(|| raw["title"].as_str())
        .unwrap_or(url);
    let inner: AnyElement = if !url.is_empty() {
        routed_link(
            format!("bk-urlsrc-{}-{ix}", kcx.ts.as_str()),
            plain_label(label, cx),
            url.to_owned(),
            kcx,
            FontWeight::NORMAL,
            cx,
        )
        .into_any_element()
    } else {
        div()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(plain_label(label, cx))
            .into_any_element()
    };
    h_flex()
        .items_center()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded_full()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            Icon::new(IconName::Globe)
                .size(px(11.))
                .text_color(theme.muted_foreground),
        )
        .child(inner)
        .into_any_element()
}

/// Slack's Block Kit `table` block: first row tinted as a header, column alignment from
/// `column_settings`, each cell either `raw_text` or nested `rich_text`.
/// Modern Block Kit `data_table` block (`F-COLLAB-719`): structured column headers + rows
/// (accepts row objects keyed by column `key`/`id` or positional cell arrays).
pub(super) fn data_table_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, header_bg, fg, muted) = (
        theme.border,
        theme.secondary,
        theme.foreground,
        theme.muted_foreground,
    );
    let columns: Vec<(String, String)> = raw["columns"]
        .as_array()
        .or_else(|| raw["headers"].as_array())
        .map(|cols| {
            cols.iter()
                .enumerate()
                .map(|(i, col)| {
                    if let Some(s) = col.as_str() {
                        (s.to_owned(), s.to_owned())
                    } else {
                        let key = col["key"]
                            .as_str()
                            .or_else(|| col["id"].as_str())
                            .or_else(|| col["name"].as_str())
                            .map(str::to_owned)
                            .unwrap_or_else(|| i.to_string());
                        let label = col["label"]
                            .as_str()
                            .or_else(|| text_of(&col["label"]))
                            .or_else(|| col["name"].as_str())
                            .or_else(|| text_of(&col["title"]))
                            .unwrap_or(&key)
                            .to_owned();
                        (key, label)
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let mut table = v_flex()
        .w_full()
        .max_w(px(760.))
        .my_1()
        .rounded_md()
        .border_1()
        .border_color(border)
        .overflow_hidden();

    if !columns.is_empty() {
        let mut hdr = h_flex()
            .w_full()
            .bg(header_bg)
            .font_weight(FontWeight::SEMIBOLD);
        for (c_ix, (_, label)) in columns.iter().enumerate() {
            hdr = hdr.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2p5()
                    .py_1p5()
                    .text_xs()
                    .text_color(fg)
                    .when(c_ix > 0, |c| c.border_l_1().border_color(border))
                    .child(plain_label(label, cx)),
            );
        }
        table = table.child(hdr);
    }

    if let Some(rows) = raw["rows"].as_array() {
        for (r_ix, row) in rows.iter().enumerate() {
            let mut row_el = h_flex()
                .w_full()
                .items_start()
                .border_t_1()
                .border_color(border);
            let cell_values: Vec<Json> = if let Some(arr) = row.as_array() {
                arr.clone()
            } else if let Some(obj) = row.as_object() {
                columns
                    .iter()
                    .map(|(k, _)| obj.get(k).cloned().unwrap_or(Json::Null))
                    .collect()
            } else {
                continue;
            };
            for (c_ix, val) in cell_values.iter().enumerate() {
                let cell_body: AnyElement = if let Some(el) = text_object(val, kcx, false, cx) {
                    el
                } else if let Some(s) = val.as_str() {
                    div()
                        .text_xs()
                        .text_color(fg)
                        .child(plain_label(s, cx))
                        .into_any_element()
                } else if val.is_number() || val.is_boolean() {
                    div()
                        .text_xs()
                        .text_color(fg)
                        .child(val.to_string())
                        .into_any_element()
                } else {
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("—")
                        .into_any_element()
                };
                row_el = row_el.child(
                    div()
                        .id(SharedString::from(format!(
                            "bk-dt-{}-{ix}-{r_ix}-{c_ix}",
                            kcx.ts.as_str()
                        )))
                        .flex_1()
                        .min_w_0()
                        .px_2p5()
                        .py_1p5()
                        .when(c_ix > 0, |c| c.border_l_1().border_color(border))
                        .child(cell_body),
                );
            }
            table = table.child(row_el);
        }
    }
    table
}

/// Compact colored `tag` badge element (`F-COLLAB-787`).
pub(super) fn tag_element(element: &Json, cx: &App) -> Div {
    let theme = cx.theme();
    let label = text_of(&element["text"])
        .or_else(|| element["label"].as_str())
        .or_else(|| element["text"].as_str())
        .unwrap_or("Tag");
    let color = match element["color"]
        .as_str()
        .or_else(|| element["style"].as_str())
        .unwrap_or_default()
    {
        "green" | "success" | "positive" => theme.success,
        "red" | "danger" | "error" => theme.danger,
        "yellow" | "warning" => theme.warning,
        "blue" | "primary" | "info" => theme.link,
        _ => theme.muted_foreground,
    };
    div()
        .px_2()
        .py(px(1.))
        .rounded_full()
        .bg(color.opacity(0.16))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(plain_label(label, cx))
}

/// Clickable `citation` pill element (`F-COLLAB-742`).
pub(super) fn citation_element(element: &Json, ix: usize, kcx: &KitCx, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let label = text_of(&element["title"])
        .or_else(|| element["label"].as_str())
        .or_else(|| element["title"].as_str())
        .or_else(|| element["url"].as_str())
        .unwrap_or("Source");
    let url = element["url"].as_str().map(str::to_owned);
    let links = kcx.links.clone();
    let mut chip = h_flex()
        .id(SharedString::from(format!(
            "bk-cite-{}-{ix}",
            kcx.ts.as_str()
        )))
        .items_center()
        .gap_1()
        .px_1p5()
        .py(px(1.))
        .rounded_sm()
        .bg(theme.link.opacity(0.12))
        .text_xs()
        .text_color(theme.link)
        .child(
            Icon::new(IconName::Link2)
                .size(px(11.))
                .text_color(theme.link),
        )
        .child(plain_label(label, cx));
    if let Some(href) = url {
        chip = chip
            .cursor_pointer()
            .hover(|c| c.underline())
            .on_click(move |_, window, cx| {
                messages::route_link(
                    links.as_ref(),
                    LinkTarget::Url(href.clone().into()),
                    window,
                    cx,
                )
            });
    }
    chip.into_any_element()
}

pub(super) fn table_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, header_bg, fg) = (theme.border, theme.secondary, theme.foreground);
    let alignments: Vec<&str> = raw["column_settings"]
        .as_array()
        .map(|cols| {
            cols.iter()
                .map(|c| c["align"].as_str().unwrap_or("left"))
                .collect()
        })
        .unwrap_or_default();
    let rows = raw["rows"].as_array();
    let mut table = v_flex()
        .w_full()
        .max_w(px(760.))
        .my_1()
        .rounded_md()
        .border_1()
        .border_color(border)
        .overflow_hidden()
        .text_xs()
        .text_color(fg);
    if let Some(rows) = rows {
        for (r_ix, row) in rows.iter().enumerate() {
            let Some(cells) = row.as_array() else {
                continue;
            };
            let is_header = r_ix == 0;
            let mut row_el = h_flex()
                .w_full()
                .items_stretch()
                .when(is_header, |r| {
                    r.bg(header_bg).font_weight(FontWeight::SEMIBOLD)
                })
                .when(r_ix > 0, |r| r.border_t_1().border_color(border));
            for (c_ix, cell) in cells.iter().enumerate() {
                let align = alignments.get(c_ix).copied().unwrap_or("left");
                let content: AnyElement = match cell["type"].as_str() {
                    Some("rich_text") => {
                        if let Ok(block) = serde_json::from_value::<Block>(cell.clone()) {
                            let cell_ts =
                                Ts::new(format!("{}-t{ix}r{r_ix}c{c_ix}", kcx.ts.as_str()));
                            let nested = kcx.nested(kcx.fixtures, kcx.kit, &cell_ts);
                            v_flex()
                                .min_w_0()
                                .children(messages::render_block(&block, 0, &nested, false, cx))
                                .into_any_element()
                        } else {
                            div()
                                .child(plain_label(cell["text"].as_str().unwrap_or_default(), cx))
                                .into_any_element()
                        }
                    }
                    _ => div()
                        .child(plain_label(cell["text"].as_str().unwrap_or_default(), cx))
                        .into_any_element(),
                };
                row_el = row_el.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2p5()
                        .py_1p5()
                        .when(c_ix > 0, |c| c.border_l_1().border_color(border))
                        .when(align == "right", |c| c.flex().justify_end())
                        .when(align == "center", |c| c.flex().justify_center())
                        .child(content),
                );
            }
            table = table.child(row_el);
        }
    }
    table
}

pub(super) fn section(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let mut text = v_flex().flex_1().min_w_0().gap_2();
    if let Some(body) = text_object(&raw["text"], kcx, false, cx) {
        text = text.child(body);
    }
    if let Some(fields) = raw["fields"].as_array() {
        // Slack lays fields out in two columns.
        for pair in fields.chunks(2) {
            text = text.child(
                h_flex()
                    .items_start()
                    .gap_4()
                    .children(pair.iter().map(|field| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .children(text_object(field, kcx, false, cx))
                    })),
            );
        }
    }
    let accessory = raw.get("accessory").map(|element| {
        let key = control_key(kcx.ts, raw, ix, element, 0);
        let rendered = if element["type"].as_str() == Some("image") {
            image_element(element, kcx, 72., cx)
        } else {
            block_element(element, raw, key, kcx, cx)
        };
        div().flex_none().child(rendered)
    });
    h_flex()
        .w_full()
        .min_w_0()
        .items_start()
        .gap_3()
        .child(text)
        .children(accessory)
}

pub(super) fn input_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let (muted, danger) = (theme.muted_foreground, theme.danger);
    let element = &raw["element"];
    let key = control_key(kcx.ts, raw, ix, element, 0);
    let block_id = raw["block_id"]
        .as_str()
        .map_or_else(|| ix.to_string(), str::to_owned);
    let inline_error = kcx
        .kit
        .and_then(|kit| kit.read(cx).block_error(&block_id).map(str::to_owned));
    v_flex()
        .w_full()
        .max_w(px(520.))
        .gap_1()
        .child(
            h_flex()
                .gap_1()
                .items_baseline()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(emoji::replace_shortcodes(
                            text_of(&raw["label"]).unwrap_or_default(),
                        )),
                )
                .when(raw["optional"].as_bool().unwrap_or(false), |label| {
                    label.child(div().text_xs().text_color(muted).child("(optional)"))
                }),
        )
        .child(block_element(element, raw, key, kcx, cx))
        .when_some(inline_error, |column, err| {
            column.child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(danger)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(12.))
                            .text_color(danger),
                    )
                    .child(err),
            )
        })
        .when_some(text_of(&raw["hint"]), |column, hint| {
            column.child(div().text_xs().text_color(muted).child(hint.to_owned()))
        })
}

/// `(label, value)` of an element's `options` and `option_groups` (preserving group headings, `F-COLLAB-762`).
pub(super) fn options(element: &Json) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = element["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| {
            Some((
                text_of(&option["text"])?.to_owned(),
                option["value"].as_str()?.to_owned(),
            ))
        })
        .collect();
    if let Some(groups) = element["option_groups"].as_array() {
        for group in groups {
            let group_label = text_of(&group["label"]).unwrap_or_default().trim();
            for option in group["options"].as_array().into_iter().flatten() {
                if let (Some(text), Some(value)) =
                    (text_of(&option["text"]), option["value"].as_str())
                {
                    let display = if group_label.is_empty() {
                        text.to_owned()
                    } else {
                        format!("{group_label} · {text}")
                    };
                    out.push((display, value.to_owned()));
                }
            }
        }
    }
    out
}

pub(super) fn choice(kcx: &KitCx, key: &str, cx: &App) -> Option<String> {
    kcx.kit
        .and_then(|kit| kit.read(cx).choices.get(key).cloned())
}

/// What Slack shows of a long attachment text until "Show more": its first five lines, at
/// most 700 characters, cut at a word.
pub(super) fn collapsed(text: &str) -> String {
    let lines = text
        .match_indices('\n')
        .nth(4)
        .map_or(text, |(end, _)| &text[..end]);
    let head = match lines.char_indices().nth(700) {
        Some((end, _)) => {
            let cut = &lines[..end];
            cut.rfind(char::is_whitespace)
                .map_or(cut, |word| &cut[..word])
        }
        None => lines,
    };
    format!("{}…", head.trim_end())
}

pub(super) fn set_choice(
    kit: Option<&Entity<BlockKitState>>,
    key: &str,
    value: &str,
    cx: &mut App,
) {
    if let Some(kit) = kit {
        kit.update(cx, |state, cx| {
            state.choices.insert(key.to_owned(), value.to_owned());
            if let Some(block_id) = key.split('/').nth(1) {
                state.clear_block_error(block_id);
            }
            cx.notify();
        });
    }
}
