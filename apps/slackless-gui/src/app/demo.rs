//! Screenshot / demo startup hooks (`SLACKLESS_GUI_DEMO`).

use super::*;

/// Screenshot/demo hook: `SLACKLESS_GUI_DEMO=browser` opens the channel browser at startup,
/// `SLACKLESS_GUI_DEMO=search:<query>` starts on the results of `<query>`,
/// `SLACKLESS_GUI_DEMO=canvas` / `=list` start in the huddle channel with the huddle bar open
/// on the Canvas / List tab. `=prefs` opens Preferences on Appearance & Themes, `=profile` the
/// first DM peer's profile, `=status` the status picker, `=switcher` the Cmd+K switcher and
/// `=menu` the title-bar user menu. `=blockkit` starts in #engineering on the deploy bot's
/// Block Kit message; `=emoji` additionally opens the composer's emoji picker. `=shortcuts`
/// opens the shortcuts sheet, `=drafts` Drafts & sent, `=side` the second workspace and
/// `=slash` the composer's slash menu. `=later` opens Later, `=pins` the pins panel,
/// `=activity` the Activity page, `=unreads` the All Unreads view, `=files` the Files view
/// (`=files:canvases` / `=files:lists` on that page), `=notify` posts a test system
/// notification, `=typing` shows two people typing in the open conversation and `=message`
/// delivers a demo direct message.
pub(super) fn demo(results: &Entity<SearchView>, window: &mut Window, cx: &mut Context<App>) {
    let Ok(demo) = std::env::var("SLACKLESS_GUI_DEMO") else {
        return;
    };
    if matches!(demo.as_str(), "blockkit" | "emoji") {
        let this = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| {
                    app.select("C0ENG".into(), cx);
                    if demo == "emoji" {
                        // Where the composer's emoji button sits in the default 1200x800 window.
                        let anchor = point(px(340.), px(730.));
                        app.open_picker(PickerTarget::Composer, anchor, window, cx);
                    }
                });
            }
        });
        return;
    }
    if matches!(
        demo.as_str(),
        "prefs"
            | "prefs:nav"
            | "prefs:advanced"
            | "prefs:audio:parakeet"
            | "prefs:audio:whisper"
            | "stealth"
            | "scheduled"
            | "secret-guard"
            | "revisions"
            | "transcript"
            | "huddle:popout"
            | "huddle:captions"
            | "profile"
            | "status"
            | "switcher"
            | "menu"
            | "shortcuts"
            | "drafts"
            | "side"
            | "slash"
            | "later"
            | "pins"
            | "activity"
            | "unreads"
            | "files"
            | "files:canvases"
            | "files:lists"
            | "threads"
            | "huddles"
            | "directory"
            | "tools"
            | "cleanup"
            | "self-profile"
            | "compose"
            | "details"
            | "wsmenu"
            | "notify"
            | "typing"
            | "message"
    ) {
        let this = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| match demo.as_str() {
                    "prefs" => app.open_preferences(Some(Section::Appearance), window, cx),
                    "prefs:nav" => app.open_preferences(Some(Section::Navigation), window, cx),
                    "prefs:advanced" => app.open_preferences(Some(Section::Advanced), window, cx),
                    "prefs:audio:parakeet" => {
                        app.preferences
                            .update(cx, |p, cx| p.set_transcription_provider(1, cx));
                        app.open_preferences(Some(Section::AudioVideo), window, cx);
                    }
                    "prefs:audio:whisper" => {
                        app.preferences
                            .update(cx, |p, cx| p.set_transcription_provider(2, cx));
                        app.open_preferences(Some(Section::AudioVideo), window, cx);
                    }
                    "stealth" => {
                        app.preferences.update(cx, |p, cx| p.set_stealth_mode(true, cx));
                        app.apply_preferences(window, cx);
                        let ch = app.channel.clone();
                        let mut ghost_collapsed = app.fixtures.local_message(
                            &ch,
                            "Det här meddelandet raderades och visas hopfällt som standard.".into(),
                            None,
                        );
                        ghost_collapsed.extra.insert(
                            crate::bridge::DELETED_EXTRA_KEY.to_owned(),
                            crate::blockkit::serde_json::Value::Bool(true),
                        );
                        let mut ghost_expanded = app.fixtures.local_message(
                            &ch,
                            "Vi sköt upp releasen till imorgon kl 10 (raderat av avsändaren)".into(),
                            None,
                        );
                        ghost_expanded.extra.insert(
                            crate::bridge::DELETED_EXTRA_KEY.to_owned(),
                            crate::blockkit::serde_json::Value::Bool(true),
                        );
                        let expand_ts = ghost_expanded.ts.clone();
                        app.messages.update(cx, |list, cx| {
                            list.append(ch.clone(), ghost_collapsed, cx);
                            list.append(ch, ghost_expanded, cx);
                            list.toggle_deleted(&expand_ts, cx);
                        });
                    }
                    "scheduled" => {
                        let ch = app.channel.clone();
                        let label = app.label(&ch);
                        let now = now_secs();
                        let ws = app.workspace;
                        app.drafts.update(cx, |d, cx| {
                            d.schedule(
                                Entry {
                                    id: Some("Q0DEMO1".to_owned()),
                                    workspace: ws,
                                    channel: ch.clone(),
                                    label: label.clone(),
                                    text: "Påminnelse: synka nya checkout-releasen innan lunch!"
                                        .to_owned(),
                                    at: now + 7_200,
                                },
                                cx,
                            );
                            d.schedule(
                                Entry {
                                    id: Some("Q0DEMO2".to_owned()),
                                    workspace: ws,
                                    channel: ch,
                                    label,
                                    text: "Good morning team — metrics look green after yesterday's deploy."
                                        .to_owned(),
                                    at: now + 86_400,
                                },
                                cx,
                            );
                        });
                        cx.notify();
                    }
                    "secret-guard" => {
                        let leaked = format!(
                            "SLACK_TOKEN={} please rotate this key",
                            ["xoxb", "1234567890", "abcdefghij123456"].join("-")
                        );
                        app.send(MessageContent::plain(leaked), window, cx);
                    }
                    "revisions" => {
                        app.open_revision_history_with(
                            "#engineering · 1764082200.000100".to_owned(),
                            vec![
                                (
                                    "Original · 10:15 AM".to_owned(),
                                    false,
                                    "Vi tar mötet klockan 10 imorgon i staging.".to_owned(),
                                ),
                                (
                                    "Edited · 10:18 AM".to_owned(),
                                    false,
                                    "Vi tar mötet klockan 14 imorgon efter deploy av checkout-order."
                                        .to_owned(),
                                ),
                                (
                                    "Deleted · 10:22 AM".to_owned(),
                                    true,
                                    "Vi tar mötet klockan 14 imorgon efter deploy av checkout-order."
                                        .to_owned(),
                                ),
                            ],
                            window,
                            cx,
                        );
                    }
                    "transcript" => {
                        let ch = ChannelId::from("C0ENG");
                        let u_alice = app
                            .fixtures
                            .users
                            .first()
                            .map(|u| u.id.clone())
                            .unwrap_or_else(|| UserId::from("U0ALICE"));
                        let u_bob = app
                            .fixtures
                            .users
                            .get(1)
                            .map(|u| u.id.clone())
                            .unwrap_or_else(|| UserId::from("U0BOB"));
                        let segs = vec![
                            slackless_core::store::HuddleTranscriptSegment {
                                id: 1,
                                channel: ch.clone(),
                                call_id: "R0DEMO123".to_owned(),
                                thread_root_ts: None,
                                speaker_id: Some(u_alice.clone()),
                                lang: Some("sv".to_owned()),
                                source: "parakeet-v3".to_owned(),
                                text: "Hej allihopa! Ska vi gå igenom nya releasen innan lunch?"
                                    .to_owned(),
                                at_ms: 1_764_082_200_000,
                            },
                            slackless_core::store::HuddleTranscriptSegment {
                                id: 2,
                                channel: ch.clone(),
                                call_id: "R0DEMO123".to_owned(),
                                thread_root_ts: None,
                                speaker_id: Some(u_bob),
                                lang: Some("en".to_owned()),
                                source: "parakeet-v3".to_owned(),
                                text: "Sounds good, let's check the checkout-order metrics first and then ship."
                                    .to_owned(),
                                at_ms: 1_764_082_212_000,
                            },
                            slackless_core::store::HuddleTranscriptSegment {
                                id: 3,
                                channel: ch.clone(),
                                call_id: "R0DEMO123".to_owned(),
                                thread_root_ts: None,
                                speaker_id: Some(u_alice.clone()),
                                lang: Some("sv".to_owned()),
                                source: "parakeet-v3".to_owned(),
                                text: "Perfekt, jag tar ett snabbt möte med teamet och pingar i kanalen."
                                    .to_owned(),
                                at_ms: 1_764_082_225_000,
                            },
                            slackless_core::store::HuddleTranscriptSegment {
                                id: 4,
                                channel: ch.clone(),
                                call_id: "R0DEMO123".to_owned(),
                                thread_root_ts: None,
                                speaker_id: Some(u_alice),
                                lang: Some("en".to_owned()),
                                source: "parakeet-v3".to_owned(),
                                text: "Also let's make sure the deploy pipeline stays green after the release."
                                    .to_owned(),
                                at_ms: 1_764_082_240_000,
                            },
                        ];
                        app.open_huddle_transcript_with(
                            dialogs::storage::HuddleTranscriptOpen {
                                call_id: "R0DEMO123".to_owned(),
                                channel: ch,
                                segments: segs,
                                highlight_segment_id: Some(2),
                                highlight_query: Some("metrics".to_owned()),
                            },
                            window,
                            cx,
                        );
                    }
                    "huddle:popout" => {
                        let ch = app.channel.clone();
                        app.join_huddle_in(ch, window, cx);
                        app.pop_out_huddle(window, cx);
                    }
                    "huddle:captions" => {
                        app.select("C0GENERAL".into(), cx);
                        app.start_huddle(window, cx);
                        let u_self = app.fixtures.self_user.clone();
                        let u_peer = app
                            .fixtures
                            .users
                            .iter()
                            .find(|u| u.id != u_self)
                            .map(|u| u.id.clone())
                            .unwrap_or_else(|| u_self.clone());
                        if let Some(dock) = app.huddle_dock.clone() {
                            dock.update(cx, |d, cx| {
                                d.set_captions_local(true, cx);
                                d.apply_data_event(
                                    &slackless_core::huddle::HuddleDataEvent::Caption {
                                        user: u_self.clone(),
                                        text: "Texten verkar vara lite utanför...".to_owned(),
                                        is_partial: false,
                                        at_ms: 1_764_082_200_000,
                                        lang: Some("sv".to_owned()),
                                    },
                                    cx,
                                );
                                d.apply_data_event(
                                    &slackless_core::huddle::HuddleDataEvent::Caption {
                                        user: u_self.clone(),
                                        text: "Hej, hej, hej, hej, hej. Vad ska du göra åt den UI-buggen för det kommer ju att gå utanför och bli en väldigt lång overflowing line in this box som måste wrappa snyggt.".to_owned(),
                                        is_partial: false,
                                        at_ms: 1_764_082_205_000,
                                        lang: Some("sv".to_owned()),
                                    },
                                    cx,
                                );
                                d.apply_data_event(
                                    &slackless_core::huddle::HuddleDataEvent::Caption {
                                        user: u_peer,
                                        text: "Modellen verkar ju vara riktigt bra i alla fall, och nu wrappar allt innanför kortet.".to_owned(),
                                        is_partial: false,
                                        at_ms: 1_764_082_210_000,
                                        lang: Some("sv".to_owned()),
                                    },
                                    cx,
                                );
                            });
                        }
                        app.pop_out_huddle(window, cx);
                    }
                    "profile" => {
                        if let Some(peer) = app.fixtures.dms.first().and_then(|c| c.user.clone()) {
                            app.open_profile(peer, cx);
                        }
                    }
                    "status" => app.open_status_picker(window, cx),
                    "switcher" => app.open_switcher(window, cx),
                    "shortcuts" => app.open_shortcuts(window, cx),
                    "drafts" => app.open_drafts(cx),
                    "side" => app.switch_workspace(1, window, cx),
                    "slash" => app
                        .composer
                        .update(cx, |composer, cx| composer.load_draft("/", cx)),
                    "later" => app.open_later(cx),
                    "pins" => app.toggle_pins(cx),
                    "activity" => app.show_nav(NavItem::Activity, cx),
                    "unreads" => app.open_unreads(window, cx),
                    "files" => app.show_nav(NavItem::Files, cx),
                    "files:canvases" | "files:lists" => {
                        app.show_nav(NavItem::Files, cx);
                        let section = if demo == "files:lists" {
                            FilesSection::Lists
                        } else {
                            FilesSection::Canvases
                        };
                        app.sidebar
                            .update(cx, |sidebar, cx| sidebar.show_files_section(section, cx));
                    }
                    "threads" => app.open_threads(cx),
                    "huddles" => app.open_huddles(cx),
                    "directory" => app.open_directory(cx),
                    "tools" => app.open_tools(cx),
                    "cleanup" => app.open_inactive_channels_dialog(window, cx),
                    "self-profile" => {
                        let me = app.fixtures.self_user.clone();
                        app.open_profile(me, cx);
                    }
                    "compose" => app.open_compose("", window, cx),
                    "details" => {
                        // The Slack Connect channel: the dialog and banner together.
                        app.select("C0PARTNER".into(), cx);
                        app.open_channel_details(
                            &"C0PARTNER".into(),
                            channel_details::DetailsTab::About,
                            window,
                            cx,
                        );
                    }
                    "wsmenu" => app
                        .sidebar
                        .update(cx, |sidebar, cx| sidebar.show_settings_menu(cx)),
                    "notify" => app.post_notification(
                        "demo",
                        "slackless",
                        "Notifications are working.",
                        window,
                        cx,
                    ),
                    "typing" => {
                        // Two DM partners typing in the open conversation, re-announced as
                        // Slack's clients do while someone keeps typing.
                        let typists: Vec<UserId> = (app.fixtures.dms.iter())
                            .filter_map(|dm| dm.user.clone())
                            .take(2)
                            .collect();
                        cx.spawn(async move |this, cx| {
                            loop {
                                let typing = this.update(cx, |app, cx| {
                                    let channel = app.channel.clone();
                                    for user in &typists {
                                        app.someone_typing(&channel, user, None, cx);
                                    }
                                });
                                if typing.is_err() {
                                    break;
                                }
                                cx.background_executor().timer(Duration::from_secs(3)).await;
                            }
                        })
                        .detach();
                    }
                    "message" => {
                        // A direct message arriving live: Slack's notification rules, banner
                        // and sound.
                        let dm = (app.fixtures.dms.iter())
                            .find(|dm| {
                                dm.id != app.channel
                                    && dm.user.as_ref() != Some(&app.fixtures.self_user)
                            })
                            .cloned();
                        if let Some(dm) = dm {
                            let message = Message {
                                ts: Ts::from("9999999999.000100"),
                                channel: Some(dm.id.clone()),
                                user: dm.user.clone(),
                                text: "This is a demo direct message.".to_owned(),
                                ..Default::default()
                            };
                            app.message_posted(&dm.id, &message, window, cx);
                        }
                    }
                    _ => {
                        app.user_menu_open = true;
                        cx.notify();
                    }
                });
            }
        });
        return;
    }
    let surface = match demo.as_str() {
        "canvas" => Some(ConversationTab::Canvas),
        "list" => Some(ConversationTab::List),
        _ => None,
    };
    if let Some(tab) = surface {
        let this = cx.weak_entity();
        window.defer(cx, move |_, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| {
                    if let Some(channel) = app.huddle.as_ref().map(|(channel, _)| channel.clone()) {
                        app.select(channel, cx);
                        app.huddle_open = true;
                    }
                    app.tab = tab;
                    cx.notify();
                });
            }
        });
    } else if demo == "browser" {
        let this = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| app.open_browser(window, cx));
            }
        });
    } else if let Some(query) = demo.strip_prefix("search:") {
        let query = query.to_owned();
        results.update(cx, |results, cx| results.run(query, cx));
        let this = cx.weak_entity();
        window.defer(cx, move |_, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| {
                    app.page = MainPage::Search;
                    cx.notify();
                });
            }
        });
    }
}
