//! GPUI rendering for [`ChannelDetails`] (About & Members tabs).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, BoxShadow, Context, Div, Focusable as _, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, div, point, px,
};
use slackless_core::NotifyLevel;
use slackless_core::types::Conversation;

use crate::avatars::{user_chip, with_external_badge};

use super::orgs::{MemberFilter, card, created_on, host_team, member_presence, org_row, own_team};
use super::{ChannelDetails, ChannelDetailsEvent, DetailsTab, EditField, MAX_ROWS, MemberRow, PAD};

impl Render for ChannelDetails {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab;
        // Slack's dialog grows with the window; the member list scrolls inside it.
        let height = (window.viewport_size().height * 0.85 - px(100.)).max(px(360.));
        v_flex()
            .h(height)
            .w_full()
            .child(self.button_row(cx))
            .child(self.render_tab_bar(cx))
            .child(match tab {
                DetailsTab::About => self.render_about_tab(cx).into_any_element(),
                DetailsTab::Members => self.render_members_tab(window, cx).into_any_element(),
            })
    }
}

impl ChannelDetails {
    /// Slack's tab row under the title: tabs 20pt apart, the active one in the text color
    /// over a 2pt underline that covers the rule; a tab's count keeps the regular weight.
    fn render_tab_bar(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, muted_foreground, foreground) =
            (theme.border, theme.muted_foreground, theme.foreground);
        let tab = self.tab;
        let member_count = if self.members.is_empty() {
            self.conversation()
                .and_then(|conversation| conversation.num_members)
                .unwrap_or(0)
        } else {
            self.rows.len() as u32
        };
        let underline = foreground.opacity(0.82);
        let tab_row = [
            (DetailsTab::About, "About", None, "details-tab-About"),
            (
                DetailsTab::Members,
                "Members",
                Some(member_count),
                "details-tab-Members",
            ),
        ]
        .map(|(entry, label, count, id)| {
            let active = entry == tab;
            h_flex()
                .id(SharedString::from(id))
                .h_full()
                .mb(px(-1.))
                .items_center()
                .gap_1()
                .cursor_pointer()
                .text_sm()
                .text_color(if active { foreground } else { muted_foreground })
                .when(active, |tab| tab.border_b_2().border_color(underline))
                .hover(|el| el.text_color(foreground))
                .child(
                    div()
                        .when(active, |label| label.font_weight(FontWeight::SEMIBOLD))
                        .child(label),
                )
                .children(count.map(|count| count.to_string()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = entry;
                    cx.notify();
                }))
        });

        h_flex()
            .items_stretch()
            .flex_none()
            .px(px(PAD))
            .gap(px(20.))
            .h(px(32.))
            .border_b_1()
            .border_color(border)
            .children(tab_row)
    }

    /// The Members tab: search input, account-type/org filter, "Add people", and scrollable list.
    fn render_members_tab(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let border = cx.theme().border;
        let needle = self.needle(cx);
        let filtering = !needle.is_empty() || self.member_filter != MemberFilter::All;
        let member_filter = &self.member_filter;
        let matching: Vec<&MemberRow> = self
            .rows
            .iter()
            .filter(|row| member_filter.keeps(row))
            .filter(|row| needle.is_empty() || row.key.contains(&needle))
            .collect();

        let find_people = self.render_find_people_field(window, cx);
        let filter_select = self.filter_select(cx);
        let add_channel = self.channel.clone();
        let add_people = crate::preferences::form_button(
            Button::new("details-add-people")
                .outline()
                .flex_none()
                .icon(IconName::UserPlus)
                .label("Add people"),
            cx,
        )
        .on_click(cx.listener(move |_, _, _, cx| {
            cx.emit(ChannelDetailsEvent::AddPeople(add_channel.clone()));
        }));

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px(px(PAD))
                    .py_4()
                    .border_b_1()
                    .border_color(border)
                    .child(find_people)
                    .child(filter_select)
                    .child(add_people),
            )
            .child(self.render_member_list_body(&matching, filtering, cx))
    }

    /// Search input with blue focus ring while the caret is in it.
    fn render_find_people_field(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (input_border, muted_foreground, ring, background) = (
            theme.input,
            theme.muted_foreground,
            theme.ring,
            theme.background,
        );
        let find_focused = self.filter.read(cx).focus_handle(cx).is_focused(window);
        let m = crate::preferences::control_metrics(cx);
        h_flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_0p5()
            .pl_2p5()
            .pr_1()
            .h(m.height)
            .rounded_md()
            // Opaque, so the focus ring's spread shows only outside the field.
            .bg(background)
            .border_1()
            .border_color(if find_focused { ring } else { input_border })
            .when(find_focused, |field| {
                field.shadow(vec![BoxShadow {
                    color: ring.opacity(0.3),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(3.),
                    inset: false,
                }])
            })
            .child(
                Icon::new(IconName::Search)
                    .flex_none()
                    .text_color(muted_foreground)
                    .size(px(14.)),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Textarea::new(&self.filter)
                        .with_size(m.input_size)
                        .appearance(false)
                        .bordered(false),
                ),
            )
    }

    fn render_member_list_body(
        &self,
        matching: &[&MemberRow],
        filtering: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let muted_foreground = cx.theme().muted_foreground;
        v_flex().flex_1().min_h_0().child(
            if let Some(note) = self.member_note(matching.is_empty(), filtering) {
                div()
                    .px(px(PAD))
                    .py_6()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child(note)
                    .into_any_element()
            } else {
                v_flex()
                    .id("details-member-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(
                        matching
                            .iter()
                            .take(MAX_ROWS)
                            .enumerate()
                            .map(|(ix, row)| self.member_row(ix, row, cx)),
                    )
                    .when(matching.len() > MAX_ROWS, |list| {
                        list.child(
                            div()
                                .px(px(PAD))
                                .py_3()
                                .text_xs()
                                .text_color(muted_foreground)
                                .child(format!("{} of {} members shown", MAX_ROWS, matching.len())),
                        )
                    })
                    .into_any_element()
            },
        )
    }

    /// The row of actions under the title: the notification level's dropdown and the huddle
    /// button (the header row's siblings, Slack shows the star unstarred there but
    /// slackless has no star backend, so it is left out).
    fn button_row(&self, cx: &mut Context<Self>) -> Div {
        let this = cx.entity().downgrade();
        let channel = self.channel.clone();
        let level = self
            .fixtures
            .notify_levels
            .get(&self.channel)
            .copied()
            .unwrap_or(NotifyLevel::Everything);
        let level_label = match level {
            NotifyLevel::Everything => "All new posts",
            NotifyLevel::Mentions => "Just mentions",
            NotifyLevel::Muted => "Mute and hide",
        };
        let notify = crate::preferences::form_button(
            Button::new("details-notify")
                .outline()
                .icon(IconName::Bell)
                .label(level_label)
                .dropdown_caret(true),
            cx,
        )
        .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
            menu = menu.label("Notify you about…");
            for (choice, label) in [
                (NotifyLevel::Everything, "All new posts"),
                (NotifyLevel::Mentions, "Just mentions"),
                (NotifyLevel::Muted, "Mute and hide"),
            ] {
                let (this, channel) = (this.clone(), channel.clone());
                menu = menu.item(PopupMenuItem::new(label).checked(choice == level).on_click(
                    move |_, _, cx| {
                        let _ = this.update(cx, |_, cx| {
                            cx.emit(ChannelDetailsEvent::SetNotifyLevel {
                                channel: channel.clone(),
                                level: choice,
                            })
                        });
                    },
                ));
            }
            menu
        });
        h_flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px(px(PAD))
            .pb_2()
            .child(notify)
            .child(
                crate::preferences::form_button(
                    Button::new("details-huddle")
                        .outline()
                        .icon(IconName::Headphones)
                        .label("Huddle"),
                    cx,
                )
                .on_click(cx.listener(|_, _, _, cx| cx.emit(ChannelDetailsEvent::StartHuddle))),
            )
    }

    /// The About tab: Slack's cards (name, topic, description, creator, permissions, actions, orgs).
    fn render_about_tab(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let conversation = self.conversation().cloned().unwrap_or_default();
        let is_channel = !conversation.is_im && !conversation.is_mpim;
        // `#general` can be renamed by workspace admins (`F-COLLAB-1236`), though it still
        // cannot be archived or deleted (`can_manage`).
        let can_rename = is_channel;
        let can_manage = is_channel && !conversation.is_general;

        let cards = v_flex()
            .w_full()
            .px(px(PAD))
            .py_4()
            .gap_2()
            .child(self.render_name_card(&conversation, can_rename, cx))
            .child(self.field_card(
                "Topic",
                EditField::Topic,
                self.topic(&conversation),
                "Add a topic",
                cx,
            ))
            .child(self.field_card(
                "Description",
                EditField::Description,
                self.purpose(&conversation),
                "Add a description",
                cx,
            ))
            .child(self.render_created_card(&conversation, cx))
            .child(self.render_permissions_card(&conversation, cx))
            .child(self.render_channel_actions(&conversation, can_manage, cx))
            .children(self.render_organizations_section(&conversation, cx));

        div()
            .id("details-about")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(cards)
    }

    /// Channel name card: editable on all channels including `#general` (`F-COLLAB-1236`), with
    /// Slack's privacy note on private / shared / default channels.
    fn render_name_card(
        &self,
        conversation: &Conversation,
        can_rename: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (border, muted_foreground) = (theme.border, theme.muted_foreground);
        let raw_name = self
            .local_name
            .clone()
            .or_else(|| conversation.name.clone())
            .unwrap_or_default();
        let editing_name = self
            .editing
            .as_ref()
            .filter(|e| e.field == EditField::Name)
            .map(|e| e.draft.clone());

        if let Some(draft) = editing_name {
            return card("Channel name", None, border)
                .when(conversation.is_general, |col| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(muted_foreground)
                            .child("Renaming the default workspace channel updates it for all members (requires admin permission)."),
                    )
                })
                .child(crate::preferences::text_input_box(&draft, false, cx))
                .child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .child(
                            crate::preferences::form_button(
                                Button::new("details-save-Name").primary().label("Save"),
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save_edit(cx))),
                        )
                        .child(
                            crate::preferences::form_button(
                                Button::new("details-cancel-Name").ghost().label("Cancel"),
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editing = None;
                                cx.notify();
                            })),
                        ),
                );
        }

        let edit_name = can_rename.then(|| {
            Button::new("details-edit-Name")
                .ghost()
                .with_size(Size::XSmall)
                .label("Edit")
                .on_click(cx.listener(|this, _, window, cx| this.edit(EditField::Name, window, cx)))
                .into_any_element()
        });
        let mut c = card("Channel name", edit_name, border);
        if conversation.is_general {
            c = c.child(
                div()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child("Default workspace channel \u{b7} Admins can rename it (cannot be archived or deleted)"),
            );
        }
        if conversation.is_private {
            let privacy_note = if conversation.is_external_connect() {
                "Private · Shared with external organizations"
            } else if conversation.is_org_shared {
                "Private · Shared across your organization"
            } else {
                "Only visible to your organization"
            };
            c = c.child(
                div()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child(privacy_note),
            );
        } else if conversation.is_external_connect() {
            c = c.child(
                div()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child("Slack Connect · Shared with external organizations"),
            );
        } else if conversation.is_org_shared {
            c = c.child(div().text_xs().text_color(muted_foreground).child(
                if conversation.is_org_mandatory {
                    "Org-wide mandatory channel"
                } else {
                    "Shared across your organization"
                },
            ));
        }
        c.child(
            h_flex()
                .items_center()
                .gap_1()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(
                    Icon::new(if conversation.is_private {
                        IconName::Lock
                    } else {
                        IconName::Hash
                    })
                    .size(px(14.)),
                )
                .child(raw_name),
        )
    }

    fn render_created_card(&self, conversation: &Conversation, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, muted_foreground) = (theme.border, theme.muted_foreground);
        let created = match conversation.created {
            Some(secs) => format!("on {}", created_on(secs)),
            None => String::new(),
        };
        let created_by = conversation
            .creator
            .as_ref()
            .map(|creator| {
                format!("{} {}", self.fixtures.user_name(creator), created)
                    .trim()
                    .to_owned()
            })
            .unwrap_or_else(|| "Unknown member".to_owned());
        card("Created by", None, border).child(
            div()
                .text_sm()
                .text_color(muted_foreground)
                .child(created_by),
        )
    }

    /// Posting & management permissions card (`F-CORE-064`).
    fn render_permissions_card(&self, conversation: &Conversation, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, muted_foreground) = (theme.border, theme.muted_foreground);
        let manager_names: Vec<String> = self
            .managers
            .iter()
            .filter_map(|id| self.fixtures.user(id))
            .map(|u| self.fixtures.name_of(u).to_owned())
            .collect();
        let posting_restricted = conversation
            .extra
            .get("properties")
            .and_then(|p| p.get("posting_restricted_to"))
            .is_some_and(|v| !v.is_null());
        let posting_policy = if conversation.is_archived {
            "Archived — nobody can post new messages."
        } else if posting_restricted {
            "Restricted — only Channel Managers & Admins can post."
        } else {
            "Everyone in this channel can post."
        };
        let member_count = if self.members.is_empty() {
            conversation.num_members.unwrap_or(0) as usize
        } else {
            self.rows.len()
        };
        let mention_policy = crate::app::mention_permission_summary(conversation, member_count);
        let managers_line = if manager_names.is_empty() {
            "Workspace Admins & Owners".to_owned()
        } else {
            manager_names.join(", ")
        };
        v_flex()
            .flex_none()
            .gap_1p5()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(border)
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Posting & management permissions"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child(posting_policy),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child(mention_policy),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child(format!("Channel managers: {managers_line}")),
            )
    }

    /// Channel management actions: Leave, Convert to private, Follow all threads, Archive/Delete.
    fn render_channel_actions(
        &self,
        conversation: &Conversation,
        can_manage: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (border, muted_foreground, link_color, accent_color) = (
            theme.border,
            theme.muted_foreground,
            theme.link,
            theme.accent,
        );
        let danger = crate::theme::danger_text(cx);

        let channel = self.channel.clone();
        let backfill_ch = self.channel.clone();
        let export_md_ch = self.channel.clone();
        let export_json_ch = self.channel.clone();
        let mut actions = v_flex()
            .flex_none()
            .gap_2()
            .child(
                v_flex()
                    .gap_1p5()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Local archive & export"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_foreground)
                            .child("Backfill full channel + thread history into SQLite, or export archived messages."),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .flex_wrap()
                            .gap_2()
                            .pt_1()
                            .child(
                                Button::new("details-backfill-channel")
                                    .outline()
                                    .with_size(Size::Small)
                                    .icon(IconName::RotateCcw)
                                    .label("Backfill history")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ChannelDetailsEvent::Backfill(backfill_ch.clone()));
                                    })),
                            )
                            .child(
                                Button::new("details-export-md")
                                    .outline()
                                    .with_size(Size::Small)
                                    .icon(IconName::Download)
                                    .label("Export Markdown\u{2026}")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ChannelDetailsEvent::Export {
                                            channel: export_md_ch.clone(),
                                            markdown: true,
                                        });
                                    })),
                            )
                            .child(
                                Button::new("details-export-json")
                                    .outline()
                                    .with_size(Size::Small)
                                    .icon(IconName::FileCode)
                                    .label("Export JSON\u{2026}")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ChannelDetailsEvent::Export {
                                            channel: export_json_ch.clone(),
                                            markdown: false,
                                        });
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .id("details-leave")
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(danger)
                    .hover(move |row| row.bg(danger.alpha(0.12)))
                    .child(Icon::new(IconName::LogOut).size(px(16.)))
                    .child("Leave channel")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ChannelDetailsEvent::Leave(channel.clone()))
                    })),
            );

        // Convert public channel or group DM to private channel (`F-CORE-049`, `F-CORE-072`).
        if (!conversation.is_private && !conversation.is_general && !conversation.is_im)
            || conversation.is_mpim
        {
            let conv_channel = self.channel.clone();
            let conv_label = if conversation.is_mpim {
                "Convert group DM to a private channel"
            } else {
                "Change to a private channel"
            };
            actions = actions.child(
                h_flex()
                    .id("details-convert-private")
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(link_color)
                    .hover(move |row| row.bg(accent_color))
                    .child(Icon::new(IconName::Lock).size(px(16.)))
                    .child(conv_label)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ChannelDetailsEvent::ConvertToPrivate(conv_channel.clone()));
                    })),
            );
        }

        // Channel-wide thread following (`F-CORE-111`).
        if !conversation.is_im {
            let follow_ch = self.channel.clone();
            let following = self.follow_all_threads;
            actions = actions.child(
                h_flex()
                    .id("details-follow-threads")
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .cursor_pointer()
                    .text_sm()
                    .hover(move |row| row.bg(accent_color))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::Bell).size(px(15.)).text_color(
                                if following {
                                    link_color
                                } else {
                                    muted_foreground
                                },
                            ))
                            .child("Follow all new threads in this channel"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if following {
                                link_color
                            } else {
                                muted_foreground
                            })
                            .child(if following { "On" } else { "Off" }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.follow_all_threads = !this.follow_all_threads;
                        cx.emit(ChannelDetailsEvent::ToggleFollowThreads(follow_ch.clone()));
                        cx.notify();
                    })),
            );
        }

        // Archive / Unarchive & Delete channel (on non-general channels).
        if can_manage {
            let archived = conversation.is_archived;
            let arch_channel = self.channel.clone();
            let del_channel = self.channel.clone();
            actions = actions
                .child(
                    h_flex()
                        .id("details-archive")
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if archived { link_color } else { danger })
                        .hover(move |row| {
                            row.bg(if archived {
                                accent_color
                            } else {
                                danger.alpha(0.12)
                            })
                        })
                        .child(Icon::new(IconName::Archive).size(px(16.)))
                        .child(if archived {
                            "Unarchive channel"
                        } else {
                            "Archive channel"
                        })
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if archived {
                                cx.emit(ChannelDetailsEvent::Unarchive(arch_channel.clone()));
                            } else {
                                cx.emit(ChannelDetailsEvent::Archive(arch_channel.clone()));
                            }
                        })),
                )
                .child(
                    h_flex()
                        .id("details-delete")
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(danger)
                        .hover(move |row| row.bg(danger.alpha(0.12)))
                        .child(Icon::new(IconName::Trash).size(px(16.)))
                        .child("Delete channel")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ChannelDetailsEvent::Delete(del_channel.clone()));
                        })),
                );
        }

        actions
    }

    /// Connected organizations & channel ownership section on Slack Connect channels
    /// (`F-COLLAB-800`, `806`, `828`, `855`).
    fn render_organizations_section(
        &self,
        conversation: &Conversation,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let has_connect = !self.orgs.is_empty()
            || conversation.is_external_connect()
            || !conversation.pending_connected_team_ids.is_empty();
        if !has_connect {
            return None;
        }
        let muted_foreground = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let host = host_team(conversation);
        let own = own_team(&self.fixtures);
        let owner_team = self.fixtures.channel_owner_team(conversation);
        let mut section = v_flex()
            .flex_none()
            .gap_1()
            .mt_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(border)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Icon::new(IconName::Building2)
                                    .size(px(14.))
                                    .text_color(muted_foreground),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Organizations"),
                            ),
                    )
                    .when_some(owner_team, |row, owner| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(muted_foreground)
                                .child(format!("Owned by {}", owner.name)),
                        )
                    }),
            );
        let mut shown_ids = std::collections::HashSet::new();
        for org in &self.orgs {
            let is_own = org.team.id == own;
            let is_host = Some(&org.team.id) == host.as_ref();
            let is_limited = conversation.is_limited_external_team(&org.team.id);
            shown_ids.insert(org.team.id.clone());
            section = section.child(org_row(
                &org.team,
                org.members,
                is_own,
                is_host,
                is_limited,
                conversation,
                muted_foreground,
            ));
        }
        // Include connected external teams reported on the conversation even before member
        // profiles load.
        for team in self.fixtures.shared_teams(conversation) {
            if shown_ids.insert(team.id.clone()) {
                let is_host = Some(&team.id) == host.as_ref();
                let is_limited = conversation.is_limited_external_team(&team.id);
                section = section.child(org_row(
                    team,
                    0,
                    false,
                    is_host,
                    is_limited,
                    conversation,
                    muted_foreground,
                ));
            }
        }
        for pending_id in &conversation.pending_connected_team_ids {
            if shown_ids.contains(pending_id) {
                continue;
            }
            let pending_name = self
                .fixtures
                .external_team_by_id(pending_id)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| pending_id.as_str().to_owned());
            section = section.child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .py_1()
                    .child(
                        Icon::new(IconName::Clock)
                            .size(px(14.))
                            .text_color(muted_foreground),
                    )
                    .child(div().text_sm().child(pending_name))
                    .child(
                        div()
                            .px_1p5()
                            .py_0p5()
                            .rounded_sm()
                            .border_1()
                            .border_color(muted_foreground.opacity(0.35))
                            .text_xs()
                            .text_color(muted_foreground)
                            .child("Pending invitation"),
                    ),
            );
        }
        Some(section)
    }

    /// One of the editable cards (topic, description): the value, or Slack's "Add a …"
    /// placeholder, with Edit at the label's right; a draft box with Save / Cancel while
    /// editing.
    fn field_card(
        &self,
        label: &str,
        field: EditField,
        value: String,
        placeholder: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (border, muted_foreground, link) = (theme.border, theme.muted_foreground, theme.link);
        let draft = self
            .editing
            .as_ref()
            .filter(|editing| editing.field == field)
            .map(|editing| editing.draft.clone());
        if let Some(draft) = draft {
            return card(label, None, border)
                .child(crate::preferences::text_input_box(&draft, true, cx))
                .child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .child(
                            crate::preferences::form_button(
                                Button::new(SharedString::from(format!("details-save-{field:?}")))
                                    .primary()
                                    .label("Save"),
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save_edit(cx))),
                        )
                        .child(
                            crate::preferences::form_button(
                                Button::new(SharedString::from(format!(
                                    "details-cancel-{field:?}"
                                )))
                                .ghost()
                                .label("Cancel"),
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editing = None;
                                cx.notify();
                            })),
                        ),
                );
        }
        let edit = Button::new(SharedString::from(format!("details-edit-{field:?}")))
            .ghost()
            .with_size(Size::XSmall)
            .label("Edit")
            .on_click(cx.listener(move |this, _, window, cx| this.edit(field, window, cx)));
        let content = if value.is_empty() {
            let add_id = match field {
                EditField::Name => "details-add-name",
                EditField::Topic => "details-add-topic",
                EditField::Description => "details-add-description",
            };
            div()
                .id(add_id)
                .text_sm()
                .cursor_pointer()
                .text_color(link)
                .child(placeholder.to_owned())
                .on_click(cx.listener(move |this, _, window, cx| this.edit(field, window, cx)))
                .into_any_element()
        } else if field == EditField::Topic {
            // The topic is mrkdwn, as in the conversation header.
            div()
                .text_sm()
                .child(crate::messages::render_preview(
                    &crate::messages::memoized_preview(&value, &self.fixtures, cx),
                    muted_foreground,
                    "details-topic".into(),
                    cx,
                ))
                .into_any_element()
        } else {
            div()
                .text_sm()
                .text_color(muted_foreground)
                .child(value)
                .into_any_element()
        };
        card(label, Some(edit.into_any_element()), border).child(content)
    }

    /// Slack's member filter beside "Find people": a 36pt select whose menu lists each
    /// choice with its count at the right, the organizations under "People from".
    fn filter_select(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let this = cx.entity().downgrade();
        let choices = Arc::clone(&self.filter_choices);
        let current = self.member_filter.clone();
        let label = choices
            .iter()
            .find(|choice| choice.filter == current)
            .map_or_else(|| SharedString::from("All"), |choice| choice.label.clone());
        let muted = cx.theme().muted_foreground;
        crate::preferences::form_button(
            Button::new("details-member-filter")
                .outline()
                .flex_none()
                .w(px(200.))
                .label(label)
                .dropdown_caret(true),
            cx,
        )
        .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
            menu = menu.min_w(px(320.));
            let mut people_from = false;
            for choice in choices.iter() {
                if matches!(choice.filter, MemberFilter::Team(_)) && !people_from {
                    people_from = true;
                    menu = menu.separator().label("People from");
                }
                let (this, filter) = (this.clone(), choice.filter.clone());
                let (label, count) = (choice.label.clone(), choice.count.to_string());
                let checked = filter == current;
                menu = menu.item(
                    PopupMenuItem::element(move |_, _| {
                        h_flex()
                            .w_full()
                            .gap_6()
                            .justify_between()
                            .child(label.clone())
                            .child(div().text_color(muted).child(count.clone()))
                    })
                    .checked(checked)
                    .on_click(move |_, _, cx| {
                        let filter = filter.clone();
                        let _ = this.update(cx, |this, cx| {
                            this.member_filter = filter;
                            cx.notify();
                        });
                    }),
                );
            }
            menu
        })
    }

    /// One member row, Slack's member list: the 36px avatar (its organization's badge when
    /// the person is external), the bold name and presence glyph, the display name with the
    /// status emoji, and the title as a second muted line. A click opens their profile.
    fn member_row(&self, ix: usize, row: &MemberRow, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (muted_foreground, foreground, hover, background, success) = (
            theme.muted_foreground,
            theme.foreground,
            theme.accent,
            theme.background,
            theme.success,
        );
        let open = row.id.clone();
        let presence = self.fixtures.presence(&row.id);
        let emoji = row
            .emoji
            .as_deref()
            .map(|code| crate::emoji::emoji_element(code, &self.fixtures, px(14.)));
        let external_org_name = row
            .team
            .as_ref()
            .and_then(|tid| self.fixtures.external_team_by_id(tid))
            .map(|team| team.name.clone());
        let border = theme.border;
        let secondary = theme.secondary;
        h_flex()
            .id(("details-member", ix))
            .flex_none()
            .items_center()
            .h(px(60.))
            .gap_3()
            .px(px(PAD))
            .cursor_pointer()
            .hover(|el| el.bg(hover))
            .child(with_external_badge(
                &self.fixtures,
                Some(&row.id),
                user_chip(&self.fixtures, &row.id, 36.),
                36.,
                background,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(foreground)
                                    .child(row.name.clone()),
                            )
                            .child(member_presence(
                                row.team.is_some(),
                                presence,
                                muted_foreground,
                                success,
                            ))
                            .when_some(external_org_name, |name_row, org_name| {
                                name_row.child(
                                    div()
                                        .flex_none()
                                        .max_w(px(140.))
                                        .truncate()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(border)
                                        .bg(secondary.opacity(0.6))
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child(org_name),
                                )
                            })
                            .children(row.handle.as_ref().map(|handle| {
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(15.))
                                    .text_color(foreground)
                                    .child(handle.clone())
                            }))
                            .children(emoji),
                    )
                    .children(row.job.as_ref().map(|job| {
                        div()
                            .truncate()
                            .text_size(px(15.))
                            .text_color(muted_foreground)
                            .child(job.clone())
                    })),
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(ChannelDetailsEvent::OpenProfile(open.clone()))
            }))
    }
}
