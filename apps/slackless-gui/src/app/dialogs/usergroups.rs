//! User-group management dialogs (`Members`, `Edit`, `Disable` / `Enable`) wired to
//! `WorkspaceCommand::{LoadUsergroupUsers, UpdateUsergroupUsers, UpdateUsergroup,
//! DisableUsergroup, EnableUsergroup}` and `Event::UsergroupUsersLoaded`.

use gpui_kit::gpui::Global;
use slackless_core::types::{UserId, Usergroup, UsergroupId};

use super::super::*;
use crate::avatars::user_chip;
use crate::directory::{
    UsergroupAction, cached_usergroup_members, record_usergroup_disabled, record_usergroup_edit,
    record_usergroup_enabled, record_usergroup_members, set_usergroup_action_handler,
};
use crate::directory_rows::usergroup_embedded_users;

/// Tracks the currently open User Group Members modal (if any) so `Event::UsergroupUsersLoaded`
/// refreshes it live without requiring a full dialog reopen.
#[derive(Default)]
struct ActiveUsergroupMembersModal(Option<gpui::WeakEntity<UsergroupMembersModal>>);

impl Global for ActiveUsergroupMembersModal {}

struct UsergroupMembersModal {
    app: gpui::WeakEntity<App>,
    fixtures: Arc<Fixtures>,
    group: Usergroup,
    members: Vec<UserId>,
    loading: bool,
    dirty: bool,
    filter: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl UsergroupMembersModal {
    fn new(
        app: gpui::WeakEntity<App>,
        fixtures: Arc<Fixtures>,
        group: Usergroup,
        initial_members: Vec<UserId>,
        loading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Search workspace members to add or remove…")
                .auto_grow(1, 1)
        });
        filter.read(cx).focus_handle(cx).focus(window, cx);
        let subscriptions = vec![cx.subscribe(
            &filter,
            |_: &mut UsergroupMembersModal, _, _: &gpui_kit::component::input::InputEvent, cx| {
                cx.notify();
            },
        )];
        Self {
            app,
            fixtures,
            group,
            members: dedup_user_ids(initial_members),
            loading,
            dirty: false,
            filter,
            _subscriptions: subscriptions,
        }
    }

    fn apply_loaded_users(&mut self, users: Vec<UserId>, cx: &mut Context<Self>) {
        self.loading = false;
        if !self.dirty {
            self.members = dedup_user_ids(users);
        }
        cx.notify();
    }

    fn toggle_member(&mut self, uid: &UserId, cx: &mut Context<Self>) {
        toggle_member_id(&mut self.members, uid);
        self.dirty = true;
        cx.notify();
    }

    fn remove_member(&mut self, uid: &UserId, cx: &mut Context<Self>) {
        if let Some(pos) = self.members.iter().position(|id| id == uid) {
            self.members.remove(pos);
            self.dirty = true;
            cx.notify();
        }
    }
}

fn dedup_user_ids(ids: Vec<UserId>) -> Vec<UserId> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn toggle_member_id(list: &mut Vec<UserId>, uid: &UserId) -> bool {
    if let Some(pos) = list.iter().position(|id| id == uid) {
        list.remove(pos);
        false
    } else {
        list.push(uid.clone());
        true
    }
}

fn filtered_workspace_candidates(
    fixtures: &Fixtures,
    needle: &str,
) -> Vec<(UserId, String, String)> {
    let needle = needle.trim().to_lowercase();
    let mut candidates: Vec<(UserId, String, String)> = fixtures
        .users
        .iter()
        .filter(|u| !u.deleted && !u.is_bot && u.id.as_str() != "USLACKBOT")
        .filter_map(|u| {
            let name = fixtures.name_of(u).to_owned();
            let handle = u.name.clone();
            let title = u.profile.title.as_deref().unwrap_or_default();
            if !needle.is_empty()
                && !name.to_lowercase().contains(&needle)
                && !handle.to_lowercase().contains(&needle)
                && !title.to_lowercase().contains(&needle)
            {
                return None;
            }
            Some((u.id.clone(), name, handle))
        })
        .collect();
    candidates.sort_by_key(|a| a.1.to_lowercase());
    candidates
}

impl Render for UsergroupMembersModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, bg, fg, hover, accent, border, chip_bg, chip_hover) = (
            theme.muted_foreground,
            theme.background,
            theme.foreground,
            theme.accent,
            theme.sidebar_accent,
            theme.border,
            theme.secondary,
            theme.secondary_hover,
        );
        let needle = self.filter.read(cx).value().to_string();
        let candidates = filtered_workspace_candidates(&self.fixtures, &needle);
        let fixtures = self.fixtures.clone();
        let member_count = self.members.len();
        let member_word = if member_count == 1 {
            "member"
        } else {
            "members"
        };

        let member_chips: Vec<_> = self
            .members
            .iter()
            .enumerate()
            .map(|(ix, uid)| {
                let uid_rm = uid.clone();
                let label = fixtures.user_name(uid).to_owned();
                h_flex()
                    .id(("ug-modal-member-chip", ix))
                    .items_center()
                    .gap_1()
                    .pl_1p5()
                    .pr_1()
                    .py_0p5()
                    .rounded_full()
                    .bg(chip_bg)
                    .text_xs()
                    .text_color(fg)
                    .child(user_chip(&fixtures, uid, 18.))
                    .child(label)
                    .child(
                        div()
                            .id(("ug-modal-member-rm", ix))
                            .px_1()
                            .rounded_full()
                            .cursor_pointer()
                            .text_color(muted)
                            .hover(move |el| el.bg(chip_hover).text_color(fg))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_member(&uid_rm, cx);
                            })),
                    )
            })
            .collect();

        let picker_rows: Vec<_> = candidates
            .into_iter()
            .take(50)
            .enumerate()
            .map(|(ix, (uid, name, handle))| {
                let is_member = self.members.contains(&uid);
                let uid_click = uid.clone();
                h_flex()
                    .id(("ug-modal-candidate", ix))
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_2p5()
                    .py_1p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(is_member, |r| r.bg(accent.opacity(0.16)))
                    .hover(move |r| r.bg(hover))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .flex_1()
                            .child(user_chip(&fixtures, &uid, 22.))
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(if is_member {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(fg)
                                    .child(name),
                            )
                            .when(!handle.is_empty(), |r| {
                                r.child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(format!("@{handle}")),
                                )
                            }),
                    )
                    .child(if is_member {
                        Icon::new(IconName::Check)
                            .size(px(14.))
                            .text_color(accent)
                            .into_any_element()
                    } else {
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Add")
                            .into_any_element()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_member(&uid_click, cx);
                    }))
            })
            .collect();

        let footer_summary = if self.loading {
            format!("{member_count} {member_word} · syncing from Slack…")
        } else if self.dirty {
            format!("{member_count} {member_word} · unsaved changes")
        } else {
            format!("{member_count} {member_word}")
        };

        v_flex()
            .gap_3()
            .p_4()
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(muted)
                                    .child(format!("Current members ({member_count})")),
                            )
                            .when(self.loading, |row| {
                                row.child(div().text_xs().text_color(muted).child("Refreshing…"))
                            }),
                    )
                    .child(
                        div()
                            .id("ug-modal-current-members")
                            .max_h(px(108.))
                            .overflow_y_scroll()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .bg(bg)
                            .child(if member_chips.is_empty() {
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(if self.loading {
                                        "Loading members from Slack…"
                                    } else {
                                        "No members selected yet. Pick teammates below."
                                    })
                                    .into_any_element()
                            } else {
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_1p5()
                                    .children(member_chips)
                                    .into_any_element()
                            }),
                    ),
            )
            .child(preferences::search_input_box(&self.filter, cx).bg(bg))
            .child(
                v_flex()
                    .id("ug-modal-picker-list")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .gap_0p5()
                    .when(picker_rows.is_empty(), |col| {
                        col.child(
                            div()
                                .py_4()
                                .text_center()
                                .text_xs()
                                .text_color(muted)
                                .child("No matching workspace members."),
                        )
                    })
                    .children(picker_rows),
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .pt_1()
                    .child(div().text_xs().text_color(muted).child(footer_summary))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                preferences::form_button(
                                    Button::new("ug-members-cancel").outline().label("Cancel"),
                                    cx,
                                )
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                preferences::form_button(
                                    Button::new("ug-members-save")
                                        .primary()
                                        .label("Save members"),
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        let group_id = this.group.id.clone();
                                        let group_name = this.group.name.clone();
                                        let users = this.members.clone();
                                        let app = this.app.clone();
                                        window.close_dialog(cx);
                                        let _ = app.update(cx, |app, cx| {
                                            app.save_usergroup_members(
                                                group_id, group_name, users, window, cx,
                                            );
                                        });
                                    },
                                )),
                            ),
                    ),
            )
    }
}

impl App {
    /// Install the GPUI global router connecting Directory user-group row actions (`Members`,
    /// `Edit`, `Disable`, `Enable`) back to this [`App`] instance.
    pub(super) fn ensure_usergroup_router(&self, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        set_usergroup_action_handler(
            move |action, window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.handle_usergroup_action(action, window, cx);
                });
            },
            cx,
        );
    }

    /// Handle a user-group action dispatched from the Directory's User groups tab.
    pub(in crate::app) fn handle_usergroup_action(
        &mut self,
        action: UsergroupAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            UsergroupAction::InspectMembers(group) => {
                self.open_usergroup_members_dialog(group, window, cx);
            }
            UsergroupAction::Edit(group) => {
                self.open_edit_usergroup_dialog(group, window, cx);
            }
            UsergroupAction::Disable(group) => {
                self.disable_usergroup_action(group, window, cx);
            }
            UsergroupAction::Enable(group) => {
                self.enable_usergroup_action(group, window, cx);
            }
        }
    }

    /// Open the User Group Members viewer/editor modal, immediately fetching fresh membership via
    /// `WorkspaceCommand::LoadUsergroupUsers` and allowing add/remove + save via
    /// `WorkspaceCommand::UpdateUsergroupUsers`.
    pub(in crate::app) fn open_usergroup_members_dialog(
        &mut self,
        group: Usergroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial_members = cached_usergroup_members(&group.id, cx)
            .unwrap_or_else(|| usergroup_embedded_users(&group));
        let loading = !self.fixtures.sample;
        if !self.fixtures.sample {
            self.bridge
                .send(WorkspaceCommand::LoadUsergroupUsers(group.id.clone()));
        }
        let title = if group.handle.is_empty() {
            format!("{} · Members", group.name)
        } else {
            format!("{} (@{}) · Members", group.name, group.handle)
        };
        let app_weak = cx.weak_entity();
        let fixtures = self.fixtures.clone();
        let modal = cx.new(|cx| {
            UsergroupMembersModal::new(
                app_weak,
                fixtures,
                group,
                initial_members,
                loading,
                window,
                cx,
            )
        });
        cx.set_global(ActiveUsergroupMembersModal(Some(modal.downgrade())));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title(title.clone()).w(px(500.)).child(modal.clone())
        });
    }

    /// Open the Edit User Group details modal (`usergroups.update`).
    pub(in crate::app) fn open_edit_usergroup_dialog(
        &mut self,
        group: Usergroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_in = cx.new(|cx| {
            let mut st = TextareaState::new(window, cx)
                .placeholder("Group name")
                .auto_grow(1, 1);
            st.set_value(group.name.clone(), window, cx);
            st
        });
        let handle_in = cx.new(|cx| {
            let mut st = TextareaState::new(window, cx)
                .placeholder("Handle without @")
                .auto_grow(1, 1);
            st.set_value(group.handle.clone(), window, cx);
            st
        });
        let desc_in = cx.new(|cx| {
            let mut st = TextareaState::new(window, cx)
                .placeholder("Purpose / description (optional)")
                .auto_grow(1, 2);
            if let Some(desc) = &group.description {
                st.set_value(desc.clone(), window, cx);
            }
            st
        });
        name_in.read(cx).focus_handle(cx).focus(window, cx);
        let app_weak = cx.weak_entity();
        let group_id = group.id.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (muted, bg) = (theme.muted_foreground, theme.background);
            let (n_c, h_c, d_c, app_c, id_c) = (
                name_in.clone(),
                handle_in.clone(),
                desc_in.clone(),
                app_weak.clone(),
                group_id.clone(),
            );
            let field = |label: &'static str, st: &Entity<TextareaState>, multi_line: bool| {
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child(label),
                    )
                    .child(preferences::text_input_box(st, multi_line, cx).bg(bg))
            };
            dialog.title("Edit user group").w(px(420.)).child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .child(field("Name", &name_in, false))
                    .child(field("Handle (@handle)", &handle_in, false))
                    .child(field("Description", &desc_in, true))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                preferences::form_button(
                                    Button::new("ug-edit-cancel").outline().label("Cancel"),
                                    cx,
                                )
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                preferences::form_button(
                                    Button::new("ug-edit-save").primary().label("Save changes"),
                                    cx,
                                )
                                .on_click(move |_, window, cx| {
                                    let name = n_c.read(cx).value().trim().to_owned();
                                    let handle = h_c
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .trim_start_matches('@')
                                        .to_owned();
                                    let description = d_c.read(cx).value().trim().to_owned();
                                    if name.is_empty() {
                                        return;
                                    }
                                    window.close_dialog(cx);
                                    let _ = app_c.update(cx, |app, cx| {
                                        app.save_usergroup_details(
                                            id_c.clone(),
                                            name,
                                            handle,
                                            description,
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                            ),
                    ),
            )
        });
    }

    fn save_usergroup_details(
        &mut self,
        id: UsergroupId,
        name: String,
        handle: String,
        description: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let desc_opt = (!description.is_empty()).then_some(description.clone());
        record_usergroup_edit(id.clone(), name.clone(), handle.clone(), desc_opt, cx);
        if !self.fixtures.sample {
            self.bridge.send(WorkspaceCommand::UpdateUsergroup {
                usergroup: id.to_string(),
                name: name.clone(),
                handle: handle.clone(),
                description,
            });
        }
        self.directory.update(cx, |_, cx| cx.notify());
        notify(
            "User group updated",
            if handle.is_empty() {
                name
            } else {
                format!("{name} (@{handle})")
            },
            window,
            cx,
        );
    }

    fn save_usergroup_members(
        &mut self,
        usergroup: UsergroupId,
        group_name: String,
        users: Vec<UserId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = users.len();
        record_usergroup_members(usergroup.clone(), users.clone(), cx);
        if !self.fixtures.sample {
            self.bridge
                .send(WorkspaceCommand::UpdateUsergroupUsers { usergroup, users });
        }
        self.directory.update(cx, |_, cx| cx.notify());
        notify(
            "Members updated",
            format!(
                "{group_name} now has {count} {}.",
                if count == 1 { "member" } else { "members" }
            ),
            window,
            cx,
        );
    }

    fn disable_usergroup_action(
        &mut self,
        group: Usergroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = group.id.clone();
        let label = if group.handle.is_empty() {
            group.name.clone()
        } else {
            format!("{} (@{})", group.name, group.handle)
        };
        record_usergroup_disabled(group, cx);
        if !self.fixtures.sample {
            self.bridge.send(WorkspaceCommand::DisableUsergroup(id));
        }
        self.directory.update(cx, |_, cx| cx.notify());
        notify("User group disabled", label, window, cx);
    }

    fn enable_usergroup_action(
        &mut self,
        group: Usergroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = group.id.clone();
        let label = if group.handle.is_empty() {
            group.name.clone()
        } else {
            format!("{} (@{})", group.name, group.handle)
        };
        record_usergroup_enabled(&id, cx);
        if !self.fixtures.sample {
            self.bridge.send(WorkspaceCommand::EnableUsergroup(id));
        }
        self.directory.update(cx, |_, cx| cx.notify());
        notify("User group enabled", label, window, cx);
    }

    /// Fold an incoming `Event::UsergroupUsersLoaded` into the live user-group membership cache,
    /// refreshing any open Members dialog and the Directory view immediately.
    pub(in crate::app) fn on_usergroup_users_loaded(
        &mut self,
        usergroup: UsergroupId,
        users: Vec<UserId>,
        cx: &mut Context<Self>,
    ) {
        record_usergroup_members(usergroup.clone(), users.clone(), cx);
        let active_modal = cx
            .try_global::<ActiveUsergroupMembersModal>()
            .and_then(|slot| slot.0.as_ref())
            .and_then(|weak| weak.upgrade());
        if let Some(modal) = active_modal {
            modal.update(cx, |m, cx| {
                if m.group.id == usergroup {
                    m.apply_loaded_users(users, cx);
                }
            });
        }
        self.directory.update(cx, |_, cx| cx.notify());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_and_dedup_user_ids_behave_cleanly() {
        let mut members = dedup_user_ids(vec![
            UserId::from("U1"),
            UserId::from("U2"),
            UserId::from("U1"),
        ]);
        assert_eq!(members, vec![UserId::from("U1"), UserId::from("U2")]);

        assert!(toggle_member_id(&mut members, &UserId::from("U3")));
        assert_eq!(
            members,
            vec![UserId::from("U1"), UserId::from("U2"), UserId::from("U3")]
        );

        assert!(!toggle_member_id(&mut members, &UserId::from("U2")));
        assert_eq!(members, vec![UserId::from("U1"), UserId::from("U3")]);
    }

    #[test]
    fn filtered_workspace_candidates_excludes_bots_and_matches_name_or_handle() {
        let fx = crate::fixtures::load();
        let all = filtered_workspace_candidates(&fx, "");
        assert!(!all.is_empty());
        assert!(all.iter().all(|(id, _, _)| id.as_str() != "USLACKBOT"));

        let first = &all[0];
        let by_name = filtered_workspace_candidates(&fx, &first.1);
        assert!(by_name.iter().any(|(id, _, _)| id == &first.0));

        let by_handle = filtered_workspace_candidates(&fx, &first.2);
        assert!(by_handle.iter().any(|(id, _, _)| id == &first.0));
    }
}
