//! Collapsible line-numbered code snippet card with lazy full-file fetching.

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, ClipboardItem, FontWeight, Global, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use slackless_core::types::{File, FileId};

use super::meta::{
    SNIPPET_COLLAPSED_LINES, download_url, file_name, snippet_hidden_lines, snippet_is_truncated,
    snippet_lines, snippet_visible_and_omitted, text_lines,
};

#[derive(Clone, Default)]
pub(super) struct SnippetCardState {
    pub(super) expanded: bool,
    pub(super) copied: bool,
    pub(super) loading: bool,
    pub(super) full_text: Option<String>,
    pub(super) error: Option<String>,
}

/// Persistent per-file snippet state stored as a GPUI [`Global`] so expanded snippets and
/// fetched full-file text survive when virtualized [`crate::messages::MessageList`] rows scroll
/// out of view and back in.
#[derive(Default)]
pub(super) struct SnippetStore {
    pub(super) entries: HashMap<FileId, SnippetCardState>,
}

impl Global for SnippetStore {}

/// Slack's collapsible line-numbered code snippet card (`Chalk_Custom.terminal`, etc.):
/// shows 5 lines collapsed with a centered `Show more (N lines)` toggle plus Copy & Download.
/// For server-truncated Slack previews (`preview_is_truncated` / `lines_more > 0`), lazily fetches
/// the full file text via [`crate::media::fetch`] on expand or Copy. Code lines participate in
/// window text selection (without selecting line numbers).
#[derive(IntoElement)]
pub(super) struct SnippetCard {
    pub(super) file: File,
    pub(super) order_base: u64,
}

impl RenderOnce for SnippetCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let file = self.file;
        let order_base = self.order_base;
        let file_id = file.id.clone();
        let id_str = file.id.to_string();
        let SnippetCardState {
            expanded,
            copied,
            loading,
            full_text,
            error,
        } = cx
            .try_global::<SnippetStore>()
            .and_then(|store| store.entries.get(&file_id))
            .cloned()
            .unwrap_or_default();
        let theme = cx.theme();
        let preview_lines = snippet_lines(&file);
        let is_truncated = snippet_is_truncated(&file, preview_lines.len());
        let has_full_text = full_text.is_some();
        let lines = match &full_text {
            Some(full) => text_lines(full),
            None => preview_lines,
        };
        let hidden_lines = snippet_hidden_lines(&file, lines.len(), has_full_text);
        let can_expand = lines.len() > SNIPPET_COLLAPSED_LINES || hidden_lines > 0 || is_truncated;
        let (visible_count, omitted_lines) = snippet_visible_and_omitted(lines.len(), expanded);
        let gutter_digits = visible_count.max(1).to_string().len().max(2);
        let title = file
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| file_name(&file));
        let kind = file
            .pretty_type
            .clone()
            .or_else(|| file.filetype.as_ref().map(|t| t.to_uppercase()));
        let dl_url = download_url(&file);
        let fetch_url = file.url_private.clone().or_else(|| dl_url.clone());
        let name = file_name(&file);
        let fallback_copy = full_text
            .clone()
            .or_else(|| file.preview.clone())
            .unwrap_or_default();
        let code_bg = crate::theme::code_block_background(cx);
        let border = theme.border;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let secondary = theme.secondary;
        let secondary_hover = theme.secondary_hover;
        let success = theme.success;

        let copy_file_id = file_id.clone();
        let copy_fetch_url = fetch_url.clone();
        let needs_copy_fetch = is_truncated && !has_full_text;
        let copy_btn = h_flex()
            .id(SharedString::from(format!("snippet-copy-{id_str}")))
            .items_center()
            .gap_1()
            .px_1p5()
            .py(px(2.))
            .rounded_sm()
            .text_xs()
            .text_color(if copied { success } else { muted })
            .cursor_pointer()
            .hover(move |b| b.bg(secondary_hover).text_color(fg))
            .child(
                Icon::new(if copied {
                    IconName::Check
                } else {
                    IconName::Copy
                })
                .size(px(12.))
                .text_color(if copied { success } else { muted }),
            )
            .child(if copied { "Copied" } else { "Copy" })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                if needs_copy_fetch
                    && let Some(fut) = copy_fetch_url
                        .clone()
                        .and_then(|url| crate::media::fetch(url, cx))
                {
                    let fid = copy_file_id.clone();
                    let fallback = fallback_copy.clone();
                    window
                        .spawn(cx, async move |cx| {
                            let res = fut.await;
                            let _ = cx.update(|window, cx| {
                                let text = match res {
                                    Ok(bytes) => {
                                        let decoded = String::from_utf8_lossy(&bytes).into_owned();
                                        let entry = cx
                                            .default_global::<SnippetStore>()
                                            .entries
                                            .entry(fid.clone())
                                            .or_default();
                                        entry.full_text = Some(decoded.clone());
                                        entry.loading = false;
                                        entry.error = None;
                                        decoded
                                    }
                                    Err(_) => fallback,
                                };
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                cx.default_global::<SnippetStore>()
                                    .entries
                                    .entry(fid)
                                    .or_default()
                                    .copied = true;
                                window.refresh();
                            });
                        })
                        .detach();
                    return;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(fallback_copy.clone()));
                cx.default_global::<SnippetStore>()
                    .entries
                    .entry(copy_file_id.clone())
                    .or_default()
                    .copied = true;
                window.refresh();
            });

        let dl_btn = dl_url.map(|url| {
            let name = name.clone();
            h_flex()
                .id(SharedString::from(format!("snippet-dl-{id_str}")))
                .items_center()
                .gap_1()
                .px_1p5()
                .py(px(2.))
                .rounded_sm()
                .text_xs()
                .text_color(muted)
                .cursor_pointer()
                .hover(move |b| b.bg(secondary_hover).text_color(fg))
                .child(
                    Icon::new(IconName::Download)
                        .size(px(12.))
                        .text_color(muted),
                )
                .child("Download")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    crate::media::download(url.clone(), name.clone(), window, cx);
                })
        });

        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::FileCode)
                            .size(px(14.))
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(title),
                    )
                    .when_some(kind, |row, k| {
                        row.child(div().text_xs().text_color(muted).child(k))
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .child(copy_btn)
                    .children(dl_btn),
            );

        let line_key = format!("snippet-{id_str}");
        let sample = lines
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let lang = crate::syntax::Lang::detect(
            file.filetype.as_deref(),
            file.name.as_deref().or(file.title.as_deref()),
            &sample,
        );
        let mut code_body = v_flex()
            .w_full()
            .px_2p5()
            .py_2()
            .font_family(mono)
            .text_size(px(12.))
            .line_height(px(18.));
        for (ix, line) in lines.iter().take(visible_count).enumerate() {
            let num = format!("{:>width$}", ix + 1, width = gutter_digits);
            let line_el: AnyElement = if line.is_empty() {
                div().flex_1().min_w_0().child(" ").into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .child(crate::messages::selectable_code(
                        &line_key,
                        ix,
                        line.clone(),
                        lang,
                        order_base + ix as u64,
                        cx,
                    ))
                    .into_any_element()
            };
            code_body = code_body.child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .gap_3()
                    .child(div().flex_none().text_color(muted.opacity(0.65)).child(num))
                    .child(line_el),
            );
        }
        if expanded && loading {
            code_body = code_body.child(
                div()
                    .pt_1()
                    .text_xs()
                    .text_color(muted)
                    .child("Loading full snippet\u{2026}"),
            );
        } else if expanded && let Some(err) = error.as_ref() {
            code_body =
                code_body.child(div().pt_1().text_xs().text_color(muted).child(err.clone()));
        }
        let truncation_notice = if expanded && !loading && omitted_lines > 0 {
            Some(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .py_1p5()
                    .border_t_1()
                    .border_color(border)
                    .bg(secondary.opacity(0.5))
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::Info).size(px(12.)).text_color(muted))
                    .child(format!(
                        "Showing first {visible_count} of {} lines ({omitted_lines} more not shown) \u{2014} copy or download for the full file.",
                        lines.len()
                    )),
            )
        } else if expanded && !loading && is_truncated && !has_full_text && error.is_none() {
            let total = lines.len() + hidden_lines;
            Some(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .py_1p5()
                    .border_t_1()
                    .border_color(border)
                    .bg(secondary.opacity(0.5))
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::Info).size(px(12.)).text_color(muted))
                    .child(format!(
                        "Showing preview ({visible_count} of {total} lines) \u{2014} download for the full file."
                    )),
            )
        } else {
            None
        };

        let toggle_row = can_expand.then(|| {
            let toggle_file_id = file_id.clone();
            let expand_fetch_url = fetch_url.clone();
            let label = if expanded {
                "Show less".to_owned()
            } else if hidden_lines > 0 {
                format!(
                    "Show more ({hidden_lines} {})",
                    if hidden_lines == 1 { "line" } else { "lines" }
                )
            } else {
                "Show more".to_owned()
            };
            h_flex()
                .w_full()
                .justify_center()
                .py_1p5()
                .border_t_1()
                .border_color(border)
                .child(
                    div()
                        .id(SharedString::from(format!("snippet-toggle-{id_str}")))
                        .px_2p5()
                        .py(px(2.))
                        .rounded(px(999.))
                        .bg(secondary)
                        .border_1()
                        .border_color(border)
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg)
                        .cursor_pointer()
                        .hover(move |b| b.bg(secondary_hover))
                        .child(label)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            let should_fetch = {
                                let entry = cx
                                    .default_global::<SnippetStore>()
                                    .entries
                                    .entry(toggle_file_id.clone())
                                    .or_default();
                                entry.expanded = !entry.expanded;
                                let fetch = entry.expanded
                                    && is_truncated
                                    && entry.full_text.is_none()
                                    && !entry.loading;
                                if fetch {
                                    entry.error = None;
                                }
                                fetch
                            };
                            window.refresh();
                            if should_fetch {
                                if let Some(fut) = expand_fetch_url
                                    .clone()
                                    .and_then(|url| crate::media::fetch(url, cx))
                                {
                                    cx.default_global::<SnippetStore>()
                                        .entries
                                        .entry(toggle_file_id.clone())
                                        .or_default()
                                        .loading = true;
                                    window.refresh();
                                    let fid = toggle_file_id.clone();
                                    window
                                        .spawn(cx, async move |cx| {
                                            let res = fut.await;
                                            let _ = cx.update(|window, cx| {
                                                let entry = cx
                                                    .default_global::<SnippetStore>()
                                                    .entries
                                                    .entry(fid)
                                                    .or_default();
                                                entry.loading = false;
                                                match res {
                                                    Ok(bytes) => {
                                                        entry.full_text = Some(
                                                            String::from_utf8_lossy(&bytes)
                                                                .into_owned(),
                                                        );
                                                        entry.error = None;
                                                    }
                                                    Err(err) => {
                                                        entry.error = Some(format!(
                                                            "Could not load full snippet ({err})."
                                                        ));
                                                    }
                                                }
                                                window.refresh();
                                            });
                                        })
                                        .detach();
                                } else {
                                    cx.default_global::<SnippetStore>()
                                        .entries
                                        .entry(toggle_file_id.clone())
                                        .or_default()
                                        .error = Some(
                                        "Full snippet requires a live Slack session (showing preview)."
                                            .to_owned(),
                                    );
                                    window.refresh();
                                }
                            }
                        }),
                )
        });

        v_flex()
            .w_full()
            .max_w(px(720.))
            .rounded(px(6.))
            .border_1()
            .border_color(border)
            .bg(code_bg)
            .overflow_hidden()
            .child(header)
            .child(code_body)
            .children(truncation_notice)
            .children(toggle_row)
    }
}
