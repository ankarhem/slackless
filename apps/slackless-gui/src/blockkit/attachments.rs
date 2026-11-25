//! Legacy Slack attachment cards, link unfurls, and message unfurls.

use super::*;

/// Legacy attachments under the message body, if any.
pub fn attachments(message: &Message, kcx: &KitCx, cx: &App) -> Option<Div> {
    if message.attachments.is_empty() {
        return None;
    }
    let show_previews = crate::preferences::display_prefs(cx).link_previews;
    let cards: Vec<Div> = message
        .attachments
        .iter()
        .enumerate()
        .filter(|(n, attachment)| {
            if !show_previews && is_page_preview_attachment(attachment) {
                return false;
            }
            if is_page_preview_attachment(attachment) {
                let key = format!("{}-unfurl-{n}", kcx.ts.as_str());
                if kcx
                    .kit
                    .is_some_and(|kit| kit.read(cx).dismissed_unfurls.contains(&key))
                {
                    return false;
                }
            }
            true
        })
        .map(|(n, attachment)| attachment_card(attachment, n, kcx, cx))
        .collect();
    if cards.is_empty() {
        return None;
    }
    // Slack's cards sit about 6px under the text above them: the body's gap alone.
    Some(v_flex().gap_2().children(cards))
}

pub(super) fn is_page_preview_attachment(attachment: &Attachment) -> bool {
    !attachment.is_msg_unfurl
        && attachment.from_url.is_some()
        && attachment.service_name.is_some()
        && attachment.color.is_none()
        && attachment
            .extra
            .get("is_app_unfurl")
            .and_then(|value| value.as_bool())
            != Some(true)
}

/// The color bar's color: `good` / `warning` / `danger`, a hex value, or the border color.
fn bar_color(color: Option<&str>, cx: &App) -> Hsla {
    let theme = cx.theme();
    match color {
        Some("good") => theme.success,
        Some("warning") => theme.warning,
        Some("danger") => theme.danger,
        Some(hex) => messages::hex_color(hex).unwrap_or(theme.border),
        None => theme.border,
    }
}

/// The attachment's own timestamp (`ts`, a number or a numeric string), in Unix seconds.
pub(super) fn attachment_secs(attachment: &Attachment) -> Option<i64> {
    attachment.ts.as_ref().and_then(|ts| {
        ts.as_i64().or_else(|| {
            ts.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .map(|secs| secs as i64)
        })
    })
}

/// `text` with Slack's link markup reduced to what it displays: `<url|label>` to `label`,
/// `<url>` to `url` (footers show links as plain muted text).
pub(super) fn link_labels(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let Some(close) = rest[open..].find('>').map(|close| open + close) else {
            break;
        };
        out.push_str(&rest[..open]);
        let inner = &rest[open + 1..close];
        out.push_str(inner.split_once('|').map_or(inner, |(_, label)| label));
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Slack's card for a shared message link (`is_msg_unfurl`): a bordered box with the author's
/// avatar, name and day, "Posted in #channel", then the message itself (its blocks when it
/// has them, else its mrkdwn text — never both, they carry the same words).
fn message_unfurl(attachment: &Attachment, n: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let fixtures = kcx.fixtures;
    let extra = |key: &str| attachment.extra.get(key).and_then(|value| value.as_str());
    let name = extra("author_subname")
        .or(attachment.author_name.as_deref())
        .unwrap_or_default()
        .to_owned();
    // The author's photo; for someone the workspace snapshot lacks, the card's own icon.
    let author = extra("author_id").map(UserId::from);
    let avatar = match (&author, attachment.author_icon.as_deref()) {
        (Some(user), Some(icon)) if fixtures.user(user).is_none() => Some(
            div()
                .size(px(36.))
                .flex_none()
                .rounded(px(7.))
                .overflow_hidden()
                .child(img(SharedString::from(icon.to_owned())).size_full()),
        ),
        (Some(user), _) => Some(avatars::user_chip(fixtures, user, 36.)),
        (None, _) => None,
    };
    let day = attachment_secs(attachment).map(messages::short_date);
    // Slack names where the shared message lives: "Thread in" for a thread's root, "From a
    // thread in" for a reply, else "Posted in", with the channel's icon and an "(archived)"
    // mark.
    let flag = |key: &str| attachment.extra.get(key).and_then(Json::as_bool) == Some(true);
    let place = if flag("is_thread_root_unfurl") {
        "Thread in"
    } else if flag("is_reply_unfurl") {
        "From a thread in"
    } else {
        "Posted in"
    };
    let posted_in = attachment
        .channel_id
        .as_ref()
        .and_then(|channel| Some((channel, fixtures.conversation(channel)?)))
        .filter(|(_, conversation)| conversation.name.is_some())
        .map(|(channel, conversation)| {
            let icon = if conversation.is_archived {
                IconName::Archive
            } else if conversation.kind() == ConversationKind::PrivateChannel {
                IconName::Lock
            } else {
                IconName::Hash
            };
            let name = conversation.name.clone().unwrap_or_default();
            // Like Slack, the channel the shared message lives in opens it.
            let channel = channel.clone();
            let links = kcx.links.clone();
            h_flex()
                .id(SharedString::from(format!(
                    "bk-{}-att-{n}-posted-in",
                    kcx.ts.as_str()
                )))
                .gap_1()
                .items_center()
                .text_size(px(13.))
                .text_color(muted)
                .cursor_pointer()
                .hover(|el| el.underline())
                .on_click(move |_, window, cx| {
                    messages::route_link(
                        links.as_ref(),
                        LinkTarget::Channel(channel.clone()),
                        window,
                        cx,
                    )
                })
                .child(place)
                .child(Icon::new(icon).text_color(muted).size(px(12.)))
                .child(if conversation.is_archived {
                    format!("{name} (archived)")
                } else {
                    name
                })
        });
    let block_ts = Ts::new(format!("{}-att{n}", kcx.ts.as_str()));
    let mut nested = kcx.nested(fixtures, kcx.kit, &block_ts);
    // A shared message's buttons belong to the original message, not this one.
    nested.on_action = None;
    let body: Vec<AnyElement> = if attachment.blocks.is_empty() {
        attachment
            .text
            .as_deref()
            .map(|text| messages::mrkdwn(text, &nested, false, false, cx))
            .map(IntoElement::into_any_element)
            .into_iter()
            .collect()
    } else {
        attachment
            .blocks
            .iter()
            .enumerate()
            .flat_map(|(ix, block)| messages::render_block(block, ix, &nested, false, cx))
            .collect()
    };
    v_flex()
        .max_w(px(640.))
        .min_w_0()
        .p_3()
        .gap_2()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        // Slack's header: the avatar (badged for another organization's member) beside two
        // 18px lines, name and day over where the message lives, together its height.
        .child(
            h_flex()
                .gap_2()
                .items_start()
                .children(avatar.map(|avatar| {
                    // Slack's card badge is smaller than a message's: ~12px on the 36px
                    // avatar, the size the helper gives a 28px one.
                    avatars::with_external_badge(
                        fixtures,
                        author.as_ref(),
                        avatar,
                        28.,
                        theme.background,
                    )
                }))
                .child(
                    v_flex()
                        .min_w_0()
                        .line_height(px(18.))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_baseline()
                                .child(div().font_weight(FontWeight::BOLD).child(name))
                                .children(
                                    day.map(|day| div().text_xs().text_color(muted).child(day)),
                                ),
                        )
                        .children(posted_in),
                ),
        )
        .children(body)
}

/// Slack's card for a plain link preview (a page's title, description, site and image): a
/// bordered box with the text on the left and the image as a thumbnail on the right.
fn link_unfurl(attachment: &Attachment, n: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, muted) = (theme.border, theme.muted_foreground);
    let title = attachment.title.as_deref().map(|title| {
        let title = plain_label(title, cx);
        let url = attachment
            .title_link
            .clone()
            .or_else(|| attachment.from_url.clone());
        match url {
            Some(url) => routed_link(
                format!("bk-{}-att-{n}-link", kcx.ts.as_str()),
                title,
                url,
                kcx,
                FontWeight::BOLD,
                cx,
            ),
            None => div()
                .id(SharedString::from(format!(
                    "bk-{}-att-{n}-link",
                    kcx.ts.as_str()
                )))
                .font_weight(FontWeight::BOLD)
                .child(title),
        }
    });
    let description = attachment
        .text
        .as_deref()
        .map(|text| messages::mrkdwn(text, kcx, true, false, cx));
    let site = attachment.service_name.as_deref().map(|name| {
        h_flex()
            .gap_1()
            .items_center()
            .text_xs()
            .text_color(muted)
            .when_some(attachment.service_icon.clone(), |row, icon| {
                row.child(
                    img(SharedString::from(icon))
                        .flex_none()
                        .size(px(14.))
                        .rounded_sm(),
                )
            })
            .child(plain_label(name, cx))
    });
    let is_video_unfurl = attachment.extra.contains_key("video_html")
        || attachment.extra.contains_key("video_html_width");
    let open_url = attachment
        .title_link
        .clone()
        .or_else(|| attachment.from_url.clone());
    let thumb = attachment
        .image_url
        .as_deref()
        .or(attachment.thumb_url.as_deref())
        .map(|url| {
            let mut inner = div()
                .relative()
                .child(picture(Some(url), "", 88., 88., kcx, cx));
            if is_video_unfurl {
                let target = open_url.clone();
                let links = kcx.links.clone();
                inner = inner.child(
                    div()
                        .id(SharedString::from(format!(
                            "bk-{}-att-{n}-play",
                            kcx.ts.as_str()
                        )))
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(
                            div()
                                .size(px(32.))
                                .rounded_full()
                                .bg(gpui_kit::gpui::hsla(0., 0., 0., 0.62))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Play)
                                        .size(px(14.))
                                        .text_color(gpui_kit::gpui::hsla(0., 0., 1., 0.95)),
                                ),
                        )
                        .when_some(target, |el, url| {
                            el.on_click(move |_, window, cx| {
                                messages::route_link(
                                    links.as_ref(),
                                    LinkTarget::Url(url.clone().into()),
                                    window,
                                    cx,
                                );
                            })
                        }),
                );
            }
            div()
                .flex_none()
                .p_2()
                .border_l_1()
                .border_color(border)
                .child(inner)
        });
    let dismiss = kcx.kit.cloned().map(|kit| {
        let key = format!("{}-unfurl-{n}", kcx.ts.as_str());
        let hover = theme.accent;
        let fg = theme.foreground;
        div()
            .id(SharedString::from(format!("bk-{key}-dismiss")))
            .absolute()
            .top_1p5()
            .right_1p5()
            .size(px(20.))
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(muted)
            .hover(move |b| b.bg(hover).text_color(fg))
            .child(Icon::new(IconName::X).size(px(12.)))
            .on_click(move |_, _, cx| {
                kit.update(cx, |state, cx| {
                    state.dismissed_unfurls.insert(key.clone());
                    cx.notify();
                });
            })
    });
    h_flex()
        .relative()
        .max_w(px(640.))
        .min_w_0()
        .items_stretch()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .p_3()
                .pr_7()
                .gap_1()
                .children(title)
                .children(description)
                .children(site),
        )
        .children(thumb)
        .children(dismiss)
}

fn attachment_card(attachment: &Attachment, n: usize, kcx: &KitCx, cx: &App) -> Div {
    if attachment.is_msg_unfurl {
        return message_unfurl(attachment, n, kcx, cx);
    }
    // A page preview (no app behind it, no colour bar): Slack's link card.
    let is_page_preview = attachment.from_url.is_some()
        && attachment.service_name.is_some()
        && attachment.color.is_none()
        && attachment
            .extra
            .get("is_app_unfurl")
            .and_then(|value| value.as_bool())
            != Some(true);
    if is_page_preview {
        return link_unfurl(attachment, n, kcx, cx);
    }
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let fixtures = kcx.fixtures;
    let mut body = v_flex().flex_1().min_w_0().gap_1();
    if let Some(author) = &attachment.author_name {
        let author_text = plain_label(author, cx);
        let label = match &attachment.author_link {
            Some(url) => routed_link(
                format!("bk-{}-att-{n}-author", kcx.ts.as_str()),
                author_text,
                url.clone(),
                kcx,
                FontWeight::SEMIBOLD,
                cx,
            )
            .text_xs()
            .into_any_element(),
            None => div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .child(author_text)
                .into_any_element(),
        };
        body = body.child(
            h_flex()
                .items_center()
                .gap_1p5()
                .when_some(attachment.author_icon.clone(), |row, icon| {
                    row.child(
                        img(SharedString::from(icon))
                            .flex_none()
                            .size(px(16.))
                            .rounded_sm(),
                    )
                })
                .child(label),
        );
    }
    if let Some(title) = &attachment.title {
        let title_text = plain_label(title, cx);
        body = body.child(match &attachment.title_link {
            Some(url) => routed_link(
                format!("bk-{}-att-{n}-title", kcx.ts.as_str()),
                title_text,
                url.clone(),
                kcx,
                FontWeight::SEMIBOLD,
                cx,
            )
            .into_any_element(),
            None => div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title_text)
                .into_any_element(),
        });
    }
    // Slack shows an attachment's blocks instead of its text when it has both.
    if let Some(text) = attachment
        .text
        .as_ref()
        .filter(|_| attachment.blocks.is_empty())
    {
        // Slack collapses text of 700+ characters or 5+ line breaks behind "Show more";
        // static cards (no kit state to expand into) show it whole.
        let key = format!("{}-att-{n}", kcx.ts.as_str());
        let long = text.chars().count() >= 700 || text.matches('\n').count() >= 5;
        let expanded = kcx
            .kit
            .is_none_or(|kit| kit.read(cx).expanded.contains(&key));
        let head;
        let shown = if long && !expanded {
            head = collapsed(text);
            &head
        } else {
            text
        };
        body = body.child(messages::mrkdwn(shown, kcx, false, false, cx));
        if let Some(kit) = kcx.kit.filter(|_| long).cloned() {
            body = body.child(
                div()
                    .id(SharedString::from(format!("bk-{key}-more")))
                    .text_color(cx.theme().link)
                    .cursor_pointer()
                    .child(if expanded { "Show less" } else { "Show more" })
                    .on_click(move |_, _, cx| {
                        kit.update(cx, |state, cx| {
                            if !state.expanded.remove(&key) {
                                state.expanded.insert(key.clone());
                            }
                            cx.notify();
                        });
                    }),
            );
        }
    }
    // Short fields pair up in two columns; long fields take a full row.
    let mut fields = attachment.fields.iter().peekable();
    while let Some(field) = fields.next() {
        let cell = |field: &slackless_core::types::message::AttachmentField| {
            v_flex()
                .flex_1()
                .min_w_0()
                .when_some(field.title.as_deref(), |cell, title| {
                    cell.child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(plain_label(title, cx)),
                    )
                })
                .when_some(field.value.as_deref(), |cell, value| {
                    cell.child(messages::mrkdwn(value, kcx, false, false, cx))
                })
        };
        let mut row = h_flex().items_start().gap_4().child(cell(field));
        if field.short {
            if let Some(next) = fields.next_if(|next| next.short) {
                row = row.child(cell(next));
            }
        }
        body = body.child(row);
    }
    let block_ts = Ts::new(format!("{}-att{n}", kcx.ts.as_str()));
    let mut nested = kcx.nested(fixtures, kcx.kit, &block_ts);
    nested.attachment = attachment.id;
    for (ix, nested_block) in attachment.blocks.iter().enumerate() {
        body = body.children(messages::render_block(nested_block, ix, &nested, false, cx));
    }
    if let Some(url) = attachment.image_url.as_deref() {
        let img_w = attachment
            .extra
            .get("image_width")
            .and_then(serde_json::Value::as_i64);
        let img_h = attachment
            .extra
            .get("image_height")
            .and_then(serde_json::Value::as_i64);
        let (w, h) = if img_w.is_some() && img_h.is_some() {
            crate::files::fit(img_w, img_h)
        } else {
            (360., 200.)
        };
        body = body.child(picture(Some(url), "attachment image", w, h, kcx, cx));
    }
    // Legacy attachment fallback text (`F-COLLAB-655`) when rich fields are all absent.
    let empty_rich = attachment.title.is_none()
        && attachment.text.is_none()
        && attachment.pretext.is_none()
        && attachment.fields.is_empty()
        && attachment.blocks.is_empty()
        && attachment.image_url.is_none();
    if empty_rich {
        if let Some(fallback) = attachment
            .fallback
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            body = body.child(messages::mrkdwn(fallback, kcx, false, false, cx));
        }
    }
    // Slack's footer line: the footer icon, then "<footer> | Yesterday at 9:25 AM", its
    // links shown as their labels.
    let stamp = attachment_secs(attachment);
    if attachment.footer.is_some() || stamp.is_some() {
        let footer = [
            attachment
                .footer
                .as_deref()
                .map(|f| plain_label(&link_labels(f), cx)),
            stamp.map(|secs| messages::day_and_time(&Ts::new(secs.to_string()))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" | ");
        body = body.child(
            h_flex()
                .gap_1()
                .items_center()
                .text_xs()
                .text_color(muted)
                .when_some(attachment.footer_icon.clone(), |row, icon| {
                    row.child(
                        img(SharedString::from(icon))
                            .flex_none()
                            .size(px(16.))
                            .rounded_sm(),
                    )
                })
                .child(footer),
        );
    }

    // Pure block attachments without a color (such as picker GIFs) render flush-left with no
    // vertical color bar, matching official Slack.
    let has_bar = attachment.color.is_some()
        || attachment.blocks.is_empty()
        || attachment.author_name.is_some()
        || attachment.title.is_some()
        || attachment.text.is_some()
        || !attachment.fields.is_empty()
        || attachment.image_url.is_some()
        || attachment.footer.is_some();

    v_flex()
        .gap_1()
        .min_w_0()
        .when_some(attachment.pretext.as_deref(), |card, pretext| {
            card.child(messages::mrkdwn(pretext, kcx, false, false, cx))
        })
        .child(if has_bar {
            h_flex()
                .items_stretch()
                .gap_3()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .w(px(4.))
                        .rounded_full()
                        .bg(bar_color(attachment.color.as_deref(), cx)),
                )
                .child(body)
                .into_any_element()
        } else {
            body.into_any_element()
        })
}
