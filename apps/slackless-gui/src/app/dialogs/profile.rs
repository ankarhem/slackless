//! Profile editor, custom status sync, profile pane, and member hover cards.

use super::super::*;

/// Inspect SCIM / admin profile field lock flags from both `team.profile.get` metadata
/// (`loaded_locks`) and `user.extra` (`is_scim_managed`, `scim_locked_fields`, `locked_fields`)
/// so Profile Edit renders read-only lock indicators instead of failing silently (`F-SHELL-005`).
pub(crate) fn detect_locked_profile_fields(
    user: Option<&slackless_core::types::User>,
    loaded_locks: &[(String, String)],
) -> HashMap<String, String> {
    let mut locked = HashMap::new();
    for (key, reason) in loaded_locks {
        let norm = key.trim().to_ascii_lowercase();
        if !norm.is_empty() {
            locked.entry(norm).or_insert_with(|| reason.clone());
        }
    }
    if let Some(user) = user {
        let scim_reason = "Managed by SCIM / identity provider".to_owned();
        if user
            .extra
            .get("is_scim_managed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            for key in ["display_name", "title", "phone"] {
                locked
                    .entry(key.to_owned())
                    .or_insert_with(|| scim_reason.clone());
            }
        }
        for array_key in ["scim_locked_fields", "locked_fields"] {
            if let Some(arr) = user
                .extra
                .get(array_key)
                .and_then(serde_json::Value::as_array)
            {
                let reason = if array_key == "scim_locked_fields" {
                    scim_reason.clone()
                } else {
                    "Locked by workspace admin".to_owned()
                };
                for item in arr.iter().filter_map(serde_json::Value::as_str) {
                    let norm = item.trim().to_ascii_lowercase();
                    if !norm.is_empty() {
                        locked.entry(norm).or_insert_with(|| reason.clone());
                    }
                }
            }
        }
        if let Some(fields) = user
            .profile
            .fields
            .as_ref()
            .and_then(serde_json::Value::as_object)
        {
            for (id, entry) in fields {
                let is_locked = entry
                    .get("is_locked")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                    || entry
                        .get("is_protected")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                if is_locked {
                    let reason = "Locked by workspace admin".to_owned();
                    locked
                        .entry(id.to_ascii_lowercase())
                        .or_insert_with(|| reason.clone());
                    if let Some(label) = entry.get("label").and_then(serde_json::Value::as_str) {
                        locked
                            .entry(label.trim().to_ascii_lowercase())
                            .or_insert(reason);
                    }
                }
            }
        }
    }
    locked
}

impl App {
    /// Sync the signed-in user's custom status (or clear it with `None`) to Slack via
    /// `users.profile.set`.
    pub(in crate::app) fn send_status_to_slack(&self, status: Option<&crate::status::UserStatus>) {
        if self.fixtures.sample {
            return;
        }
        let update = match status {
            Some(s) => slackless_core::api::ProfileUpdate {
                status_text: Some(s.text.clone()),
                status_emoji: Some(format!(":{}:", s.emoji.trim_matches(':'))),
                status_expiration: Some(s.clears_at().unwrap_or(0)),
                ..Default::default()
            },
            None => slackless_core::api::ProfileUpdate {
                status_text: Some(String::new()),
                status_emoji: Some(String::new()),
                status_expiration: Some(0),
                ..Default::default()
            },
        };
        self.bridge.send(WorkspaceCommand::UpdateProfile { update });
    }

    /// Open Slack's "Edit your profile" dialog for the signed-in user (display name, title,
    /// phone, pronouns), persisting via `users.profile.set`.
    pub(in crate::app) fn open_profile_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = self.fixtures.user(&self.fixtures.self_user).cloned();
        let initial_display = me
            .as_ref()
            .and_then(|u| u.profile.display_name.clone())
            .unwrap_or_default();
        let initial_title = me
            .as_ref()
            .and_then(|u| u.profile.title.clone())
            .unwrap_or_default();
        let initial_phone = me
            .as_ref()
            .and_then(|u| u.profile.phone.clone())
            .unwrap_or_default();
        let (loaded_pronouns, loaded_fields) = self
            .profile
            .read(cx)
            .loaded_profile(&self.fixtures.self_user);
        let loaded_locks = self
            .profile
            .read(cx)
            .loaded_locked_fields(&self.fixtures.self_user);
        let locked_map = detect_locked_profile_fields(me.as_ref(), &loaded_locks);
        let initial_show_pronouns = self.preferences.read(cx).settings().show_pronouns;
        let show_pronouns_state = Rc::new(Cell::new(initial_show_pronouns));
        let initial_pronouns = loaded_pronouns
            .or_else(|| me.as_ref().and_then(|u| u.profile.pronouns.clone()))
            .or_else(|| {
                self.fixtures
                    .profile_extras
                    .get(&self.fixtures.self_user)
                    .and_then(|e| e.pronouns.clone())
            })
            .unwrap_or_default();
        let initial_tz = me.as_ref().and_then(|u| u.tz.clone()).unwrap_or_default();

        let display_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("Display name")
                .auto_grow(1, 1);
            s.set_value(initial_display.clone(), window, cx);
            s
        });
        let title_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("What I do (e.g. Developer)")
                .auto_grow(1, 1);
            s.set_value(initial_title.clone(), window, cx);
            s
        });
        let phone_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("Phone number")
                .auto_grow(1, 1);
            s.set_value(initial_phone.clone(), window, cx);
            s
        });
        let pronouns_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("Pronouns (optional)")
                .auto_grow(1, 1);
            s.set_value(initial_pronouns.clone(), window, cx);
            s
        });
        let tz_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("Time zone (e.g. Europe/Stockholm)")
                .auto_grow(1, 1);
            s.set_value(initial_tz.clone(), window, cx);
            s
        });

        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, cx| {
            let theme = cx.theme();
            let (muted, border, card_bg) = (
                theme.muted_foreground,
                theme.border,
                theme.secondary.opacity(0.45),
            );
            let lock_reason = |keys: &[&str]| -> Option<String> {
                keys.iter().find_map(|k| locked_map.get(*k).cloned())
            };
            let display_lock = lock_reason(&["display_name", "display name"]);
            let title_lock = lock_reason(&["title"]);
            let phone_lock = lock_reason(&["phone"]);
            let pronouns_lock = lock_reason(&["pronouns"]);
            let tz_lock = lock_reason(&["tz", "time zone", "timezone"]);

            let field = |label: &'static str,
                         state: &Entity<TextareaState>,
                         current_val: &str,
                         lock: Option<&str>| {
                if let Some(reason) = lock {
                    let shown = if current_val.trim().is_empty() {
                        "(Managed by organization)".to_owned()
                    } else {
                        current_val.to_owned()
                    };
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child(label),
                                )
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_1()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded_sm()
                                        .bg(card_bg)
                                        .text_size(px(10.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child(
                                            Icon::new(IconName::Lock)
                                                .size(px(10.))
                                                .text_color(muted),
                                        )
                                        .child("Locked"),
                                ),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .px_2p5()
                                .py_1p5()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .bg(card_bg)
                                .text_sm()
                                .text_color(muted)
                                .child(shown),
                        )
                        .child(div().text_xs().text_color(muted).child(reason.to_owned()))
                } else {
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(muted)
                                .child(label),
                        )
                        .child(preferences::text_input_box(state, false, cx))
                }
            };
            let show_pronouns_val = show_pronouns_state.get();
            let show_pronouns_toggle = {
                let flag = show_pronouns_state.clone();
                gpui_kit::component::checkbox::Checkbox::new("profile-show-pronouns")
                    .label("Display pronouns on my profile")
                    .checked(show_pronouns_val)
                    .on_click(move |checked, window, _| {
                        flag.set(*checked);
                        window.refresh();
                    })
            };
            let this_save = this.clone();
            let (d_save, t_save, p_save, pr_save, tz_save, fields_save, locks_save) = (
                display_in.clone(),
                title_in.clone(),
                phone_in.clone(),
                pronouns_in.clone(),
                tz_in.clone(),
                loaded_fields.clone(),
                loaded_locks.clone(),
            );
            let (d_locked, t_locked, p_locked, pr_locked, tz_locked) = (
                display_lock.is_some(),
                title_lock.is_some(),
                phone_lock.is_some(),
                pronouns_lock.is_some(),
                tz_lock.is_some(),
            );
            let show_pronouns_save = show_pronouns_state.clone();
            let this_photo = this.clone();
            let max_body_h = (window.viewport_size().height * 0.78).max(px(360.));
            dialog.title("Edit your profile").w(px(460.)).child(
                v_flex()
                    .id("edit-profile-body")
                    .max_h(max_body_h)
                    .overflow_y_scroll()
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .gap_3()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Profile photo"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child("PNG, JPG, GIF or WebP up to 10 MB"),
                                    ),
                            )
                            .child(
                                preferences::form_button(
                                    Button::new("profile-upload-photo")
                                        .outline()
                                        .icon(IconName::Image)
                                        .label("Upload photo\u{2026}"),
                                    cx,
                                )
                                .on_click(move |_, window, cx| {
                                    let this_u = this_photo.clone();
                                    let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
                                        files: true,
                                        directories: false,
                                        multiple: false,
                                        prompt: Some("Choose profile photo".into()),
                                    });
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
                                                .map(|n| n.to_string_lossy().into_owned())
                                                .unwrap_or_else(|| "avatar.png".to_owned());
                                            // Off the UI thread; the 10 MB limit is checked on
                                            // the file's size before anything is read.
                                            let bytes = match cx
                                                .background_executor()
                                                .spawn(read_profile_photo(path))
                                                .await
                                            {
                                                Ok(bytes) => bytes,
                                                Err(PhotoReadError::TooLarge) => {
                                                    let _ = cx.update(|window, cx| {
                                                        notify(
                                                            "Photo too large",
                                                            "Choose an image smaller than 10 MB.",
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                    return;
                                                }
                                                Err(PhotoReadError::Unreadable(err)) => {
                                                    let _ = cx.update(|window, cx| {
                                                        notify(
                                                            "Could not read photo",
                                                            err.to_string(),
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                    return;
                                                }
                                            };
                                            let _ = cx.update(|window, cx| {
                                                let _ = this_u.update(cx, |app, _| {
                                                    if !app.fixtures.sample {
                                                        app.bridge.send(
                                                            WorkspaceCommand::SetUserPhoto {
                                                                filename: filename.clone(),
                                                                bytes,
                                                            },
                                                        );
                                                    }
                                                });
                                                notify(
                                                    "Profile photo",
                                                    format!("Uploading {filename}\u{2026}"),
                                                    window,
                                                    cx,
                                                );
                                            });
                                        })
                                        .detach();
                                }),
                            ),
                    )
                    .child(field(
                        "Display name",
                        &display_in,
                        &initial_display,
                        display_lock.as_deref(),
                    ))
                    .child(field(
                        "Title",
                        &title_in,
                        &initial_title,
                        title_lock.as_deref(),
                    ))
                    .child(field(
                        "Phone",
                        &phone_in,
                        &initial_phone,
                        phone_lock.as_deref(),
                    ))
                    .child(
                        v_flex()
                            .gap_1p5()
                            .child(field(
                                "Pronouns",
                                &pronouns_in,
                                &initial_pronouns,
                                pronouns_lock.as_deref(),
                            ))
                            .child(show_pronouns_toggle)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(if show_pronouns_val {
                                        "Visible on your profile by default."
                                    } else {
                                        "Hidden from teammates on your profile preview."
                                    }),
                            ),
                    )
                    .child(field("Time zone", &tz_in, &initial_tz, tz_lock.as_deref()))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .pt_2()
                            .child(
                                preferences::form_button(
                                    Button::new("profile-edit-cancel").outline().label("Cancel"),
                                    cx,
                                )
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                preferences::form_button(
                                    Button::new("profile-edit-save")
                                        .primary()
                                        .label("Save Changes"),
                                    cx,
                                )
                                .on_click(move |_, window, cx| {
                                    let pronouns_val = pr_save.read(cx).value().trim().to_owned();
                                    let tz_val = tz_save.read(cx).value().trim().to_owned();
                                    let show_pr = show_pronouns_save.get();
                                    let update = slackless_core::api::ProfileUpdate {
                                        display_name: (!d_locked)
                                            .then(|| d_save.read(cx).value().trim().to_owned()),
                                        title: (!t_locked)
                                            .then(|| t_save.read(cx).value().trim().to_owned()),
                                        phone: (!p_locked)
                                            .then(|| p_save.read(cx).value().trim().to_owned()),
                                        pronouns: (!pr_locked).then(|| {
                                            if show_pr {
                                                pronouns_val.clone()
                                            } else {
                                                String::new()
                                            }
                                        }),
                                        tz: (!tz_locked && !tz_val.is_empty()).then_some(tz_val),
                                        ..Default::default()
                                    };
                                    if let Some(app) = this_save.upgrade() {
                                        app.update(cx, |app, cx| {
                                            let me_id = app.fixtures.self_user.clone();
                                            app.profile.update(cx, |profile, cx| {
                                                profile.set_show_pronouns(show_pr, cx);
                                                profile.set_user_profile(
                                                    me_id,
                                                    (!pronouns_val.is_empty())
                                                        .then_some(pronouns_val),
                                                    fields_save.clone(),
                                                    locks_save.clone(),
                                                    cx,
                                                );
                                            });
                                            let mut settings =
                                                app.preferences.read(cx).settings().clone();
                                            if settings.show_pronouns != show_pr {
                                                settings.show_pronouns = show_pr;
                                                app.preferences.update(cx, |prefs, cx| {
                                                    prefs.load_settings(
                                                        settings.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                });
                                                if let Some(write) =
                                                    app.bridge.remember_settings(&settings)
                                                {
                                                    cx.background_executor().spawn(write).detach();
                                                }
                                            }
                                            if !app.fixtures.sample {
                                                app.bridge.send(WorkspaceCommand::UpdateProfile {
                                                    update,
                                                });
                                            }
                                            notify(
                                                "Profile updated",
                                                "Your profile changes were saved.",
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                    window.close_dialog(cx);
                                }),
                            ),
                    ),
            )
        });
    }

    /// Slack's profile pane. Opened from an open thread it stacks over the thread, which
    /// stays loaded under it: the transcript drops the thread's tint while the profile
    /// shows, and the profile's back chevron returns to the thread.
    pub(in crate::app) fn open_profile(&mut self, user: UserId, cx: &mut Context<Self>) {
        self.close_hover_card(cx);
        let over_thread = matches!(
            self.right_pane,
            RightPane::Thread | RightPane::Profile { over_thread: true }
        );
        if !self.fixtures.sample {
            self.bridge
                .send(WorkspaceCommand::FetchUserProfile(user.clone()));
        }
        self.profile
            .update(cx, |profile, cx| profile.open(user, over_thread, cx));
        self.set_right_pane(RightPane::Profile { over_thread }, cx);
        cx.notify();
    }

    // ---------------------------------------------------------------------------
    // Slack's member profile hover card (`hover_card.rs`): dwell open, grace close.
    // ---------------------------------------------------------------------------

    /// The pointer entered a profile hover trigger (an @mention pill or an author name):
    /// dwell before showing, so a sweeping pointer never flashes the card. A user the
    /// snapshot lacks (an external member) is resolved right away, so the card has a name
    /// by the time the dwell ends.
    pub(crate) fn profile_hover_entered(
        &mut self,
        trigger: hover_card::Trigger,
        cx: &mut Context<Self>,
    ) {
        let unchanged = self.profile_hover.trigger.as_ref() == Some(&trigger);
        let same_open_card = self
            .profile_hover
            .card
            .as_ref()
            .is_some_and(|card| card.key == trigger.key && card.user == trigger.user);
        self.profile_hover.grace = None;
        self.profile_hover.trigger = Some(trigger.clone());
        if self.fixtures.user(&trigger.user).is_none() {
            self.resolve_users(std::iter::once(trigger.user.clone()));
        }
        if unchanged || same_open_card {
            return;
        }
        let (key, user) = (trigger.key.clone(), trigger.user.clone());
        self.profile_hover.dwell = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(hover_card::DWELL_MS))
                .await;
            let _ = this.update(cx, |app, cx| app.hover_dwell_elapsed(&key, &user, cx));
        }));
    }

    /// The pointer left the trigger registered under `key`. A leave from any other element
    /// (the pointer crossed straight onto a newer trigger before this frame's hover diff)
    /// is stale and drops.
    pub(crate) fn profile_hover_left(&mut self, key: &SharedString, cx: &mut Context<Self>) {
        if !self
            .profile_hover
            .trigger
            .as_ref()
            .is_some_and(|trigger| trigger.key == *key)
        {
            return;
        }
        self.profile_hover.trigger = None;
        self.profile_hover.dwell = None; // left before the dwell: no card
        self.hover_card_maybe_close(cx);
    }

    /// The dwell elapsed with the pointer still on `key`'s trigger: show the card.
    pub(in crate::app) fn hover_dwell_elapsed(
        &mut self,
        key: &SharedString,
        user: &UserId,
        cx: &mut Context<Self>,
    ) {
        let Some(trigger) = self
            .profile_hover
            .trigger
            .clone()
            .filter(|trigger| trigger.key == *key && trigger.user == *user)
        else {
            return; // the pointer moved on (or left) before the dwell ended
        };
        self.profile_hover.card = Some(trigger);
        self.profile_hover.grace = None;
        cx.notify();
    }

    /// The grace elapsed without the pointer returning to the trigger or into the card.
    pub(in crate::app) fn hover_grace_elapsed(&mut self, cx: &mut Context<Self>) {
        if self.profile_hover.trigger.is_some() || self.profile_hover.inside {
            return; // the pointer came back before the grace closed the card
        }
        self.profile_hover.grace = None;
        if self.profile_hover.card.take().is_some() {
            cx.notify();
        }
    }

    /// The pointer entered / left the open card: entering cancels a pending close (the
    /// pointer crossed the gap from its trigger); leaving starts the grace.
    pub(in crate::app) fn hover_card_hovered(&mut self, inside: bool, cx: &mut Context<Self>) {
        if self.profile_hover.inside == inside {
            return;
        }
        self.profile_hover.inside = inside;
        if inside {
            self.profile_hover.grace = None;
        } else {
            self.hover_card_maybe_close(cx);
        }
    }

    /// Start the card's close grace, unless the pointer is still on its trigger or inside the
    /// card, or a grace is already running.
    pub(in crate::app) fn hover_card_maybe_close(&mut self, cx: &mut Context<Self>) {
        if self.profile_hover.card.is_none()
            || self.profile_hover.trigger.is_some()
            || self.profile_hover.inside
            || self.profile_hover.grace.is_some()
        {
            return;
        }
        self.profile_hover.grace = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(hover_card::GRACE_MS))
                .await;
            let _ = this.update(cx, |app, cx| app.hover_grace_elapsed(cx));
        }));
    }

    /// Close the card outright: a click outside, Escape, a conversation switch, or an
    /// action opened from it. The trigger stays, so the card does not come back until the
    /// pointer re-enters it.
    pub(in crate::app) fn close_hover_card(&mut self, cx: &mut Context<Self>) {
        let closed = self.profile_hover.card.take().is_some();
        self.profile_hover.inside = false;
        self.profile_hover.grace = None;
        self.profile_hover.dwell = None;
        if closed {
            cx.notify();
        }
    }
}

/// Slack's `users.setPhoto` limit.
const MAX_PROFILE_PHOTO_BYTES: u64 = 10 * 1024 * 1024;

/// Why a picked profile photo was not read: over the 10 MB limit (decided on the file's size
/// before anything is read), or unreadable.
enum PhotoReadError {
    TooLarge,
    Unreadable(std::io::Error),
}

/// Read a picked profile photo off the UI thread.
async fn read_profile_photo(path: std::path::PathBuf) -> Result<Vec<u8>, PhotoReadError> {
    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() > MAX_PROFILE_PHOTO_BYTES => Err(PhotoReadError::TooLarge),
        _ => match std::fs::read(&path) {
            Ok(bytes) if bytes.len() as u64 <= MAX_PROFILE_PHOTO_BYTES => Ok(bytes),
            Ok(_) => Err(PhotoReadError::TooLarge),
            Err(err) => Err(PhotoReadError::Unreadable(err)),
        },
    }
}
