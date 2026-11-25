//! GPUI rendering for [`ChannelDetails`] (About, Members, Integrations, Settings tabs).

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

use crate::avatars::{user_chip, with_external_badge};

use super::orgs::{MemberFilter, card, created_on, host_team, member_presence, org_row, own_team};
use super::{ChannelDetails, ChannelDetailsEvent, DetailsTab, EditField, MAX_ROWS, MemberRow, PAD};

impl Render for ChannelDetails {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, input_border, muted_foreground, foreground, ring, background) = (
            theme.border,
            theme.input,
            theme.muted_foreground,
            theme.foreground,
            theme.ring,
            theme.background,
        );
        let tab = self.tab;
        let member_count = if self.members.is_empty() {
            self.conversation()
                .and_then(|conversation| conversation.num_members)
                .unwrap_or(0)
        } else {
            self.rows.len() as u32
        };

        // Slack's tab row under the title: tabs 20pt apart, the active one in the text color
        // over a 2pt underline that covers the rule; a tab's count keeps the regular weight.
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

        // Slack's dialog grows with the window; the member list scrolls inside it.
        let height = (window.viewport_size().height * 0.85 - px(100.)).max(px(360.));
        v_flex()
            .h(height)
            .w_full()
            .child(self.button_row(cx))
            .child(
                h_flex()
                    .items_stretch()
                    .flex_none()
                    .px(px(PAD))
                    .gap(px(20.))
                    .h(px(32.))
                    .border_b_1()
                    .border_color(border)
                    .children(tab_row),
            )
            .child(match tab {
                DetailsTab::About => self.about_tab(cx).into_any_element(),
                DetailsTab::Members => {
                    let needle = self.needle(cx);
                    // Slack's field carries the blue focus ring while the caret is in it
                    // (the dialog opens with it focused).
                    let find_focused = self.filter.read(cx).focus_handle(cx).is_focused(window);
                    let filtering = !needle.is_empty() || self.member_filter != MemberFilter::All;
                    let member_filter = &self.member_filter;
                    let matching: Vec<&MemberRow> = self
                        .rows
                        .iter()
                        .filter(|row| member_filter.keeps(row))
                        .filter(|row| needle.is_empty() || row.key.contains(&needle))
                        .collect();
                    let m = crate::preferences::control_metrics(cx);
                    let find_people = h_flex()
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
                        );
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
                        .child(v_flex().flex_1().min_h_0().child(
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
                                                .child(format!(
                                                    "{} of {} members shown",
                                                    MAX_ROWS,
                                                    matching.len()
                                                )),
                                        )
                                    })
                                    .into_any_element()
                            },
                        ))
                        .into_any_element()
                }
            })
    }
}

impl ChannelDetails {
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

    /// The About tab: Slack's cards.
    fn about_tab(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (border, muted_foreground, link_color, accent_color) = (
            theme.border,
            theme.muted_foreground,
            theme.link,
            theme.accent,
        );
        let conversation = self.conversation().cloned().unwrap_or_default();
        let mut cards = v_flex().w_full().px(px(PAD)).py_4().gap_2();

        // Channel name: editable on channels other than #general, with Slack's privacy note on private channels.
        let is_channel = !conversation.is_im && !conversation.is_mpim;
        let can_manage = is_channel && !conversation.is_general;
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
        let name_card = if let Some(draft) = editing_name {
            card("Channel name", None, border)
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
                )
        } else {
            let edit_name = can_manage.then(|| {
                Button::new("details-edit-Name")
                    .ghost()
                    .with_size(Size::XSmall)
                    .label("Edit")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.edit(EditField::Name, window, cx)),
                    )
                    .into_any_element()
            });
            let mut c = card("Channel name", edit_name, border);
            if conversation.is_private {
                c = c.child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child("Only visible to your organization"),
                );
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
        };
        cards = cards.child(name_card);

        // Topic and description: the cards Slack lets a member edit.
        cards = cards.child(self.field_card(
            "Topic",
            EditField::Topic,
            self.topic(&conversation),
            "Add a topic",
            cx,
        ));
        cards = cards.child(self.field_card(
            "Description",
            EditField::Description,
            self.purpose(&conversation),
            "Add a description",
            cx,
        ));

        // Created by: the creator's name and the creation date.
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
        cards = cards.child(
            card("Created by", None, border).child(
                div()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child(created_by),
            ),
        );

        // Posting & management permissions card (`F-CORE-064`).
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
        let managers_line = if manager_names.is_empty() {
            "Workspace Admins & Owners".to_owned()
        } else {
            manager_names.join(", ")
        };
        cards = cards.child(
            v_flex()
                .flex_none()
                .gap_1p5()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
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
                        .child(format!("Channel managers: {managers_line}")),
                ),
        );

        // Leave channel, Slack's red row at the end of the cards.
        let channel = self.channel.clone();
        cards = cards.child(
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
                .text_color(crate::theme::danger_text(cx))
                .hover(|row| row.bg(crate::theme::danger_text(cx).alpha(0.12)))
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
            cards = cards.child(
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
            cards = cards.child(
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
                    .border_color(cx.theme().border)
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
            let danger = crate::theme::danger_text(cx);
            cards = cards
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

        // Organizations, on a Slack Connect channel: one row per organization (`rebuild`
        // builds the lines for shared channels only).
        if !self.orgs.is_empty() {
            let host = host_team(&conversation);
            let mut section = v_flex().flex_none().gap_1().mt_2().child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Organizations"),
            );
            // Slack lists the other organizations only, not the viewer's own.
            let own = own_team(&self.fixtures);
            for org in self.orgs.iter().filter(|org| org.team.id != own) {
                let is_host = Some(&org.team.id) == host.as_ref();
                section = section.child(org_row(
                    &org.team,
                    org.members,
                    is_host,
                    &conversation,
                    muted_foreground,
                ));
            }
            cards = cards.child(section);
        }
        div()
            .id("details-about")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(cards)
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
                .child(crate::messages::mrkdwn_preview(
                    &value,
                    &self.fixtures,
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

    /// One member row: avatar, name, and Slack's External marker for people from another
    /// organization; a click opens their profile.
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
                            .items_center()
                            .gap_2()
                            .child(
                                div()
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
                            .children(row.handle.as_ref().map(|handle| {
                                div()
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
