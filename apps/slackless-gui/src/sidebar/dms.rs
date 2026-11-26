//! Direct messages inbox column.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::types::{ChannelId, Conversation, Edited, Message, Ts};

use super::{Sidebar, SidebarEvent, count_pill};
use crate::avatars::conversation_avatar;
use crate::fixtures::Fixtures;
use crate::messages::{Preview, list_stamp, preview_model, render_preview};

thread_local! {
    /// The DM list's previews: by DM, the latest message (its ts and edit) each was modeled
    /// for, against the workspace data and emoji preference the memo holds. A frame renders
    /// them without parsing the message again.
    static PREVIEWS: RefCell<PreviewMemo> = RefCell::default();
}

#[derive(Default)]
struct PreviewMemo {
    fixtures: Option<Arc<Fixtures>>,
    emoji_text: bool,
    previews: HashMap<ChannelId, (Ts, Option<Edited>, Rc<Preview>)>,
}

impl Sidebar {
    /// The rail's DMs destination, Slack's DM list: the collapsible "Direct messages" header
    /// with the "Unreads" filter toggle and the compose pen, a "Find a DM…" field, then one
    /// row per DM, latest activity first — presence or stacked group avatar, bold name with
    /// its relative stamp, and a muted preview of the latest message ([`Fixtures::latest`] in
    /// live mode, the history in fixtures).
    pub(super) fn dm_list(&self, watermark: Option<&Ts>, cx: &mut Context<Self>) -> Div {
        let query = self.dm_find_query.as_str();
        let mut dms: Vec<(&Conversation, Option<&Message>)> = self
            .fixtures
            .dms
            .iter()
            // Slack lists apps' and Slackbot's DMs under Apps, not here.
            .filter(|dm| {
                !dm.user.as_ref().is_some_and(|user| {
                    matches!(user.as_str(), "USLACKBOT" | "USLACK")
                        || self.fixtures.user(user).is_some_and(|u| u.is_bot)
                })
            })
            .map(|dm| (dm, self.dm_latest(dm)))
            .filter(|&(dm, _)| {
                query.is_empty()
                    || self
                        .fixtures
                        .conversation_label(dm)
                        .to_lowercase()
                        .contains(query)
            })
            .filter(|&(dm, _)| !self.dm_unreads_only || self.unread_in(dm, watermark).count > 0)
            .collect();
        dms.sort_by(|a, b| {
            b.1.map(|message| &message.ts)
                .cmp(&a.1.map(|message| &message.ts))
        });
        let border = cx.theme().sidebar_border;
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, (dm, last)) in dms.into_iter().enumerate() {
            // Slack separates the rows with hairlines, not gaps.
            if index > 0 {
                rows.push(div().h(px(1.)).flex_none().bg(border).into_any_element());
            }
            rows.push(self.dm_row(dm, last, watermark, cx).into_any_element());
        }
        let collapsed = SharedString::from("dm-list");
        v_flex()
            .child(self.dm_header(cx))
            .when(!self.collapsed.contains(&collapsed), |list| {
                list.child(self.find_field(&self.dm_find, cx))
                    .child(v_flex().pt_1().pb_2().children(rows))
            })
    }

    /// The DM's latest top-level message: the freshest history the views hold (fixture mode,
    /// or live mode's open conversation), else the snapshot's per-DM latest message.
    pub(super) fn dm_latest(&self, dm: &Conversation) -> Option<&Message> {
        if let Some(fresh) = self.fresh_latest.get(&dm.id) {
            return fresh.as_ref();
        }
        self.fixtures
            .messages
            .get(&dm.id)
            .and_then(|history| history.last())
            .or_else(|| self.fixtures.latest.get(&dm.id))
    }

    /// The DM list's header: the collapsible "Direct messages" title, Slack's "Unreads"
    /// filter toggle and the compose pen on the right.
    pub(super) fn dm_header(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let dim = theme.sidebar_foreground.opacity(0.6);
        let hover = theme.sidebar_foreground.opacity(0.08);
        let border = theme.sidebar_border;
        let toggle = cx.weak_entity();
        let on_unreads = move |checked: &bool, _: &mut Window, cx: &mut gpui_kit::gpui::App| {
            toggle
                .update(cx, |sidebar: &mut Sidebar, cx| {
                    sidebar.dm_unreads_only = *checked;
                    cx.notify();
                })
                .ok();
        };
        h_flex()
            .px_4()
            .pt_3()
            .pb_2()
            .gap_2()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .id("dm-list-title")
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|title| title.bg(hover))
                    // Slack's title reads "Direct messages ⌄": the chevron follows it.
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child("Direct messages"),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .text_color(dim)
                            .size(px(14.)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let key = SharedString::from("dm-list");
                        if !this.collapsed.remove(&key) {
                            this.collapsed.insert(key);
                        }
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(dim)
                    .child("Unreads")
                    .child(
                        Switch::new("dm-unreads")
                            .accessibility_label("Unreads")
                            .with_size(Size::XSmall)
                            .checked(self.dm_unreads_only)
                            .on_click(on_unreads),
                    ),
            )
            .child(
                self.header_icon("dm-compose", IconName::SquarePen, dim, hover)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenCompose))),
            )
    }

    pub(super) fn dm_row(
        &self,
        dm: &Conversation,
        last: Option<&Message>,
        watermark: Option<&Ts>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let selected = dm.id == self.selected;
        let unread = self.unread_in(dm, watermark);
        let (fg, dim, preview_fg) = (
            theme.sidebar_foreground,
            theme.sidebar_foreground.opacity(0.6),
            theme.sidebar_foreground.opacity(0.9),
        );
        // Slack's DM list marks the open DM with a gray wash and keeps its text colors.
        let selected_bg = theme.sidebar.blend(theme.sidebar_foreground.opacity(0.1));
        let ring = if selected { selected_bg } else { theme.sidebar };
        let hover = theme.sidebar_foreground.opacity(0.08);
        // The self-DM reads "Name (you)" here, "(you)" in regular weight, as Slack's DM list
        // shows it; every other surface keeps the plain label.
        let label = self.fixtures.conversation_label(dm);
        let is_self = dm.is_im
            && dm
                .user
                .as_ref()
                .is_some_and(|user| *user == self.fixtures.self_user);
        let preview = last.map(|message| {
            // Formatting, links and emoji render as in the transcript.
            let emoji_text = crate::preferences::display_prefs(cx).emoji_text_mode;
            let model = dm_preview(&self.fixtures, &dm.id, message, emoji_text);
            render_preview(&model, dim, format!("dm-preview-{}", dm.id), cx)
        });
        let stamp = last
            .map(|message| list_stamp(&message.ts))
            .unwrap_or_default();
        let id = dm.id.clone();
        h_flex()
            .id(SharedString::from(format!("dm-{id}")))
            .px_2()
            .py_3()
            .gap_2()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            // Like Slack, the selected row keeps its colour under the pointer.
            .when(selected, |row| row.bg(selected_bg))
            .when(!selected, |row| row.hover(|row| row.bg(hover)))
            .child(conversation_avatar(&self.fixtures, dm, 36., ring, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .text_color(fg)
                                    .child(
                                        div().truncate().font_weight(FontWeight::BOLD).child(label),
                                    )
                                    .when(is_self, |name| {
                                        name.child(div().flex_none().child("(you)"))
                                    }),
                            )
                            .child(div().flex_none().text_xs().text_color(dim).child(stamp)),
                    )
                    // Slack's preview is as large as the name and only slightly dimmer.
                    .child(div().line_clamp(2).text_color(preview_fg).children(preview)),
            )
            .when(unread.count > 0, |row| {
                row.child(count_pill(
                    unread.mentions.max(unread.count),
                    unread.mentions > 0,
                    cx,
                ))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select_row(id.clone(), cx)))
    }
}

/// The preview of `message`, the DM `dm`'s latest, modeled once while it stays the latest and
/// unedited.
fn dm_preview(
    fixtures: &Arc<Fixtures>,
    dm: &ChannelId,
    message: &Message,
    emoji_text: bool,
) -> Rc<Preview> {
    PREVIEWS.with(|memo| {
        let mut memo = memo.borrow_mut();
        let current = memo
            .fixtures
            .as_ref()
            .is_some_and(|known| known.shares_data(fixtures))
            && memo.emoji_text == emoji_text;
        if !current {
            *memo = PreviewMemo {
                fixtures: Some(fixtures.clone()),
                emoji_text,
                previews: HashMap::new(),
            };
        }
        if let Some((ts, edited, preview)) = memo.previews.get(dm)
            && *ts == message.ts
            && *edited == message.edited
        {
            return preview.clone();
        }
        // Slack prefixes only your own messages, in group DMs too.
        let author = match &message.user {
            Some(user) if *user == fixtures.self_user => "You: ",
            _ => "",
        };
        let text = format!("{author}{}", dm_preview_text(fixtures, message));
        let preview = Rc::new(preview_model(&text, fixtures, emoji_text));
        memo.previews.insert(
            dm.clone(),
            (message.ts.clone(), message.edited.clone(), preview.clone()),
        );
        preview
    })
}

/// What the DM list previews of `message`: its raw mrkdwn, as the transcript renders it (a
/// link typed literally arrives escaped; parsing its unescaped plain text again would draw a
/// live link to the first address labelled with the second), or the file line of a bare share.
pub(super) fn dm_preview_text(fixtures: &Fixtures, message: &Message) -> String {
    if message.text.trim().is_empty() {
        fixtures.preview(message)
    } else {
        message.text.clone()
    }
}
