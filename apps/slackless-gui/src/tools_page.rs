//! Native `Agents & tools` view (`F-COLLAB-314`, `F-COLLAB-466`, `F-COLLAB-795`):
//! replaces the Home fallback on the left rail's `Agents & tools` tile with three tabbed
//! workspace panels:
//! - **Apps**: installed workspace bots & apps with instant search and "Open DM" / "App Home".
//! - **Commands**: workspace slash commands (`client.appCommands`) & built-ins with click-to-use.
//! - **Slack Connect**: connected external organizations and shared external channels.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use slackless_core::types::{ChannelId, Conversation, Team, UserId};

use crate::avatars::user_chip;
use crate::fixtures::Fixtures;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolsTab {
    Apps,
    Commands,
    Connect,
}

pub enum ToolsPageEvent {
    OpenAppDm(UserId),
    OpenChannel(ChannelId),
    InsertCommand(String),
    Close,
}

pub struct ToolsPage {
    fixtures: Arc<Fixtures>,
    tab: ToolsTab,
    filter: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ToolsPageEvent> for ToolsPage {}

impl ToolsPage {
    pub fn new(fixtures: Arc<Fixtures>, window: &mut Window, cx: &mut App) -> Entity<ToolsPage> {
        cx.new(|cx| {
            let filter = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Filter apps, commands, or connected organizations\u{2026}")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![
                cx.subscribe(&filter, |_: &mut ToolsPage, _, _: &InputEvent, cx| {
                    cx.notify()
                }),
            ];
            ToolsPage {
                fixtures,
                tab: ToolsTab::Apps,
                filter,
                _subscriptions: subscriptions,
            }
        })
    }

    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    fn needle(&self, cx: &Context<Self>) -> String {
        self.filter.read(cx).value().trim().to_ascii_lowercase()
    }

    fn apps(&self, needle: &str) -> Vec<(Option<UserId>, String, String)> {
        let mut list: Vec<(Option<UserId>, String, String)> = self
            .fixtures
            .users
            .iter()
            .filter(|u| u.is_bot && !u.deleted && u.id.as_str() != "USLACKBOT")
            .map(|u| {
                let title = u
                    .profile
                    .title
                    .as_deref()
                    .filter(|t| !t.is_empty())
                    .unwrap_or("Workspace app")
                    .to_owned();
                (Some(u.id.clone()), u.display_name().to_owned(), title)
            })
            .collect();
        if let Ok(bots) = self.fixtures.bots.read() {
            for bot in bots.values() {
                if bot.name.is_empty() || bot.deleted {
                    continue;
                }
                let already = list
                    .iter()
                    .any(|(_, name, _)| name.eq_ignore_ascii_case(&bot.name));
                if !already {
                    list.push((None, bot.name.clone(), "Workspace bot".to_owned()));
                }
            }
        }
        list.retain(|(_, name, title)| {
            needle.is_empty()
                || name.to_ascii_lowercase().contains(needle)
                || title.to_ascii_lowercase().contains(needle)
        });
        list.sort_by_key(|a| a.1.to_ascii_lowercase());
        list
    }

    fn connect_channels(&self, needle: &str) -> Vec<&Conversation> {
        self.fixtures
            .channels
            .iter()
            .chain(&self.fixtures.dms)
            .filter(|c| c.is_ext_shared || c.is_shared || c.is_org_shared)
            .filter(|c| {
                needle.is_empty()
                    || self
                        .fixtures
                        .conversation_label(c)
                        .to_ascii_lowercase()
                        .contains(needle)
            })
            .collect()
    }

    fn external_orgs(&self, needle: &str) -> Vec<&Team> {
        let own = self.fixtures.workspaces.first().map(|t| &t.id);
        self.fixtures
            .teams
            .iter()
            .filter(|t| Some(&t.id) != own)
            .filter(|t| {
                needle.is_empty()
                    || t.name.to_ascii_lowercase().contains(needle)
                    || t.domain.to_ascii_lowercase().contains(needle)
            })
            .collect()
    }
}

impl Render for ToolsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, fg, link, hover) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.link,
            theme.accent,
        );
        let needle = self.needle(cx);
        let tab = self.tab;

        let tabs = [
            (ToolsTab::Apps, "Installed apps"),
            (ToolsTab::Commands, "Slash commands"),
            (ToolsTab::Connect, "Slack Connect"),
        ]
        .map(|(t, label)| {
            let active = t == tab;
            div()
                .id(SharedString::from(format!("tools-tab-{label}")))
                .h_full()
                .flex()
                .items_center()
                .px_1()
                .cursor_pointer()
                .text_sm()
                .text_color(if active { fg } else { muted })
                .when(active, |el| {
                    el.font_weight(FontWeight::SEMIBOLD)
                        .border_b_2()
                        .border_color(link)
                })
                .hover(|el| el.text_color(fg))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = t;
                    cx.notify();
                }))
        });

        let body = match tab {
            ToolsTab::Apps => {
                let apps = self.apps(&needle);
                let mut col = v_flex().gap_1p5().p_4();
                if apps.is_empty() {
                    col = col.child(
                        div()
                            .py_6()
                            .text_sm()
                            .text_color(muted)
                            .child("No installed apps match your filter."),
                    );
                } else {
                    for (ix, (uid_opt, name, title)) in apps.into_iter().take(60).enumerate() {
                        let avatar = match &uid_opt {
                            Some(uid) => user_chip(&self.fixtures, uid, 32.).into_any_element(),
                            None => div()
                                .size(px(32.))
                                .rounded_md()
                                .bg(theme.secondary)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(IconName::Bot).size(px(16.)).text_color(muted))
                                .into_any_element(),
                        };
                        col = col.child(
                            h_flex()
                                .id(("tools-app-row", ix))
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .hover(|el| el.bg(hover))
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_3()
                                        .min_w_0()
                                        .child(avatar)
                                        .child(
                                            v_flex()
                                                .min_w_0()
                                                .child(
                                                    h_flex()
                                                        .items_center()
                                                        .gap_1p5()
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .truncate()
                                                                .child(name),
                                                        )
                                                        .child(
                                                            div()
                                                                .px_1()
                                                                .rounded(px(3.))
                                                                .bg(theme.secondary)
                                                                .text_size(px(10.))
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(muted)
                                                                .child("APP"),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .truncate()
                                                        .child(title),
                                                ),
                                        ),
                                )
                                .when_some(uid_opt, |row, uid| {
                                    row.child(
                                        Button::new(("tools-app-open", ix))
                                            .outline()
                                            .with_size(Size::XSmall)
                                            .label("Open")
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                cx.emit(ToolsPageEvent::OpenAppDm(uid.clone()));
                                            })),
                                    )
                                }),
                        );
                    }
                }
                col
            }
            ToolsTab::Commands => {
                let mut cmds: Vec<(String, String, String)> = crate::slash::BUILTINS
                    .iter()
                    .map(|c| {
                        (
                            format!("/{}", c.name),
                            c.usage.to_owned(),
                            c.description.to_owned(),
                        )
                    })
                    .collect();
                for c in &self.fixtures.app_commands {
                    let slash = if c.name.starts_with('/') {
                        c.name.clone()
                    } else {
                        format!("/{}", c.name)
                    };
                    if !cmds.iter().any(|(n, _, _)| n == &slash) {
                        cmds.push((slash, c.usage.clone(), c.description.clone()));
                    }
                }
                cmds.retain(|(name, usage, desc)| {
                    needle.is_empty()
                        || name.to_ascii_lowercase().contains(&needle)
                        || usage.to_ascii_lowercase().contains(&needle)
                        || desc.to_ascii_lowercase().contains(&needle)
                });
                let mut col = v_flex().gap_1p5().p_4();
                if cmds.is_empty() {
                    col = col.child(
                        div()
                            .py_6()
                            .text_sm()
                            .text_color(muted)
                            .child("No slash commands match your filter."),
                    );
                } else {
                    for (ix, (name, usage, desc)) in cmds.into_iter().take(80).enumerate() {
                        let cmd_insert = format!("{name} ");
                        col = col.child(
                            h_flex()
                                .id(("tools-cmd-row", ix))
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .hover(|el| el.bg(hover))
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .font_family(theme.mono_font_family.clone())
                                                        .child(name),
                                                )
                                                .when(!usage.is_empty(), |row| {
                                                    row.child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(muted)
                                                            .child(usage),
                                                    )
                                                }),
                                        )
                                        .when(!desc.is_empty(), |c| {
                                            c.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .truncate()
                                                    .child(desc),
                                            )
                                        }),
                                )
                                .child(
                                    Button::new(("tools-cmd-use", ix))
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .label("Use")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(ToolsPageEvent::InsertCommand(
                                                cmd_insert.clone(),
                                            ));
                                        })),
                                ),
                        );
                    }
                }
                col
            }
            ToolsTab::Connect => {
                let orgs = self.external_orgs(&needle);
                let channels = self.connect_channels(&needle);
                let mut col = v_flex().gap_3().p_4();
                if !orgs.is_empty() {
                    col = col
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(muted)
                                .child(format!("Connected organizations ({})", orgs.len())),
                        )
                        .child(h_flex().flex_wrap().gap_2().children(orgs.into_iter().map(
                            |org| {
                                h_flex()
                                    .items_center()
                                    .gap_1p5()
                                    .px_2p5()
                                    .py_1()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(border)
                                    .bg(theme.secondary.opacity(0.35))
                                    .child(
                                        Icon::new(IconName::Globe).size(px(13.)).text_color(link),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(org.name.clone()),
                                    )
                            },
                        )));
                }
                col = col.child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .child(format!("Shared external channels ({})", channels.len())),
                );
                if channels.is_empty() {
                    col = col.child(
                        div()
                            .py_4()
                            .text_sm()
                            .text_color(muted)
                            .child("No Slack Connect channels found."),
                    );
                } else {
                    for (ix, ch) in channels.into_iter().take(60).enumerate() {
                        let cid = ch.id.clone();
                        let label = ch
                            .name
                            .as_deref()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .unwrap_or_else(|| {
                                self.fixtures
                                    .conversation_label(ch)
                                    .trim_start_matches('#')
                                    .trim()
                                    .to_owned()
                            });
                        let icon = if ch.is_private {
                            IconName::Lock
                        } else {
                            IconName::Hash
                        };
                        col = col.child(
                            h_flex()
                                .id(("tools-connect-ch", ix))
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .cursor_pointer()
                                .hover(|el| el.bg(hover))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(ToolsPageEvent::OpenChannel(cid.clone()));
                                }))
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(Icon::new(icon).size(px(14.)).text_color(muted))
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(label),
                                        ),
                                )
                                .child(div().text_xs().text_color(muted).child("External")),
                        );
                    }
                }
                col
            }
        };

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .h(px(46.))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child(Icon::new(IconName::LayoutGrid).size(px(18.)))
                            .child("Agents & tools"),
                    )
                    .child(
                        Button::new("tools-close")
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ToolsPageEvent::Close))),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px_4()
                    .gap_5()
                    .h(px(38.))
                    .border_b_1()
                    .border_color(border)
                    .children(tabs),
            )
            .child(
                h_flex()
                    .flex_none()
                    .mx_4()
                    .mt_3()
                    .items_center()
                    .gap_2()
                    .px_2p5()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(Icon::new(IconName::Search).size(px(14.)).text_color(muted))
                    .child(
                        div().flex_1().min_w_0().child(
                            Textarea::new(&self.filter)
                                .appearance(false)
                                .bordered(false),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .id("tools-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}
