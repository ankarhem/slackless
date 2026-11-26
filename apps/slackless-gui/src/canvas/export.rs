//! Canvas Print export helpers and print-availability policy guard (`F-COLLAB-182`,
//! `F-COLLAB-184`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use slackless_core::api::CanvasMeta;

use crate::canvas::mentions::{CanvasInlineSegment, parse_inline_canvas_segments};
use crate::canvas::model::{CanvasBlock, CanvasDoc};
use crate::canvas::parse::resolve_canvas_emoji;

/// Counter ensuring concurrent print/PDF exports never collide on temp HTML paths.
static EXPORT_SEQ: AtomicU64 = AtomicU64::new(0);

/// Maximum age before temporary canvas export HTML files are pruned.
const TEMP_HTML_MAX_AGE: Duration = Duration::from_secs(60 * 60);
/// Maximum number of recent temporary canvas export HTML files retained on disk.
const TEMP_HTML_MAX_FILES: usize = 8;

/// Remove stale temporary canvas print HTML files (`slackless-print-*.html`) in `dir` older
/// than `max_age` or exceeding `max_files` newest entries.
pub(super) fn prune_stale_export_files(dir: &Path, max_age: Duration, max_files: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    let mut kept: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !(name.starts_with("slackless-print-") && name.ends_with(".html")) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let expired = now.duration_since(modified).is_ok_and(|age| age > max_age);
        if expired {
            let _ = std::fs::remove_file(&path);
        } else {
            kept.push((modified, path));
        }
    }
    if kept.len() > max_files {
        kept.sort_by_key(|a| std::cmp::Reverse(a.0));
        for (_, path) in kept.into_iter().skip(max_files) {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn next_temp_html_path(prefix: &str, stem: &str) -> PathBuf {
    let seq = EXPORT_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{stem}-{}-{seq}.html", std::process::id()))
}

/// Whether workspace/canvas policy allows printing or exporting this canvas as PDF (`F-COLLAB-184`).
pub fn canvas_print_allowed(meta: Option<&CanvasMeta>) -> bool {
    meta.map(|m| m.canvas_printing_enabled).unwrap_or(true)
}

/// Escape text safely for HTML body & attribute contexts.
pub fn html_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Format inline canvas text (resolving emoji & rendering mention pills) into escaped HTML.
fn format_inline_html(text: &str) -> String {
    let segments = parse_inline_canvas_segments(text);
    if segments.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for seg in segments {
        match seg {
            CanvasInlineSegment::Text(t) => {
                out.push_str(&html_escape(&resolve_canvas_emoji(&t)));
            }
            CanvasInlineSegment::UserMention { user_id, label } => {
                let display = label.unwrap_or(user_id);
                let _ = write!(
                    out,
                    "<span class=\"mention\">@{}</span>",
                    html_escape(&display)
                );
            }
            CanvasInlineSegment::ChannelMention { channel_id, name } => {
                let display = name.unwrap_or(channel_id);
                let _ = write!(
                    out,
                    "<span class=\"mention\">#{}</span>",
                    html_escape(&display)
                );
            }
            CanvasInlineSegment::UsergroupMention { handle, .. } => {
                let _ = write!(
                    out,
                    "<span class=\"mention\">@{}</span>",
                    html_escape(&handle)
                );
            }
            CanvasInlineSegment::BroadcastMention(kind) => {
                let _ = write!(
                    out,
                    "<span class=\"mention broadcast\">@{}</span>",
                    html_escape(&kind)
                );
            }
        }
    }
    out
}

/// Render a [`CanvasDoc`] into a clean, self-contained HTML5 document with `@media print` CSS
/// suitable for native printing (`F-COLLAB-182`).
pub fn canvas_to_printable_html(doc: &CanvasDoc) -> String {
    let escaped_title = html_escape(&resolve_canvas_emoji(&doc.title));
    let escaped_meta = html_escape(&doc.meta);

    let mut body = String::new();
    let _ = writeln!(body, "<header class=\"doc-header\">");
    let _ = writeln!(body, "  <h1>{escaped_title}</h1>");
    if !doc.meta.is_empty() {
        let _ = writeln!(body, "  <p class=\"doc-meta\">{escaped_meta}</p>");
    }
    let _ = writeln!(body, "</header>");

    for block in &doc.blocks {
        match block {
            CanvasBlock::Heading { level, text } => {
                let tag = match level {
                    1 => "h1",
                    2 => "h2",
                    _ => "h3",
                };
                let _ = writeln!(body, "<{tag}>{}</{tag}>", format_inline_html(text));
            }
            CanvasBlock::Paragraph(text) => {
                let _ = writeln!(body, "<p>{}</p>", format_inline_html(text));
            }
            CanvasBlock::Bullets(items) => {
                body.push_str("<ul>\n");
                for item in items {
                    let _ = writeln!(body, "  <li>{}</li>", format_inline_html(item));
                }
                body.push_str("</ul>\n");
            }
            CanvasBlock::NumberedList(items) => {
                body.push_str("<ol>\n");
                for item in items {
                    let _ = writeln!(body, "  <li>{}</li>", format_inline_html(item));
                }
                body.push_str("</ol>\n");
            }
            CanvasBlock::Checklist(items) => {
                body.push_str("<ul class=\"checklist\">\n");
                for (done, item) in items {
                    let checked_attr = if *done { " checked" } else { "" };
                    let done_cls = if *done { " class=\"done\"" } else { "" };
                    let _ = writeln!(
                        body,
                        "  <li{done_cls}><input type=\"checkbox\" disabled{checked_attr} /> {}</li>",
                        format_inline_html(item)
                    );
                }
                body.push_str("</ul>\n");
            }
            CanvasBlock::CodeBlock(code) => {
                let _ = writeln!(body, "<pre><code>{}</code></pre>", html_escape(code));
            }
            CanvasBlock::Quote(text) => {
                body.push_str("<blockquote>\n");
                for line in text.lines() {
                    let _ = writeln!(body, "  <p>{}</p>", format_inline_html(line));
                }
                body.push_str("</blockquote>\n");
            }
            CanvasBlock::Table { header, rows } => {
                body.push_str("<table>\n<thead><tr>");
                for cell in header {
                    let _ = write!(body, "<th>{}</th>", format_inline_html(cell));
                }
                body.push_str("</tr></thead>\n<tbody>\n");
                for row in rows {
                    body.push_str("<tr>");
                    for cell in row {
                        let _ = write!(body, "<td>{}</td>", format_inline_html(cell));
                    }
                    body.push_str("</tr>\n");
                }
                body.push_str("</tbody>\n</table>\n");
            }
            CanvasBlock::Image { url, alt, .. } => {
                let esc_url = html_escape(url);
                let esc_alt = html_escape(&resolve_canvas_emoji(alt));
                let _ = writeln!(body, "<figure>");
                let _ = writeln!(body, "  <img src=\"{esc_url}\" alt=\"{esc_alt}\" />");
                if !alt.is_empty() {
                    let _ = writeln!(body, "  <figcaption>{esc_alt}</figcaption>");
                }
                let _ = writeln!(body, "</figure>");
            }
            CanvasBlock::MediaEmbed {
                kind,
                title,
                url,
                duration,
                ..
            } => {
                let dur = duration
                    .as_deref()
                    .map(|d| format!(" ({})", html_escape(d)))
                    .unwrap_or_default();
                let _ = writeln!(
                    body,
                    "<div class=\"embed-card\"><strong>{}:</strong> <a href=\"{}\">{}</a>{dur}</div>",
                    kind.label(),
                    html_escape(url),
                    html_escape(&resolve_canvas_emoji(title)),
                );
            }
            CanvasBlock::ProfileCard {
                user_id,
                display_name,
                title,
                handle,
            } => {
                let name = display_name
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(user_id.as_str());
                let sub = title
                    .as_deref()
                    .or(handle.as_deref())
                    .map(|s| format!(" — {}", html_escape(s)))
                    .unwrap_or_default();
                let _ = writeln!(
                    body,
                    "<div class=\"embed-card\"><strong>{}</strong>{sub}</div>",
                    html_escape(name),
                );
            }
            CanvasBlock::DocEmbed {
                kind, title, url, ..
            } => {
                let _ = writeln!(
                    body,
                    "<div class=\"embed-card\"><strong>{}:</strong> <a href=\"{}\">{}</a></div>",
                    kind.label(),
                    html_escape(url),
                    html_escape(&resolve_canvas_emoji(title)),
                );
            }
            CanvasBlock::Divider => {
                body.push_str("<hr />\n");
            }
        }
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<title>{escaped_title}</title>
<style>
  :root {{ color-scheme: light; }}
  body {{
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    max-width: 760px;
    margin: 32px auto;
    padding: 0 24px;
    color: #1d1c1d;
    line-height: 1.55;
    font-size: 14px;
  }}
  .doc-header {{ border-bottom: 1px solid #ddd; padding-bottom: 12px; margin-bottom: 20px; }}
  .doc-meta {{ color: #616061; font-size: 12px; margin: 4px 0 0; }}
  h1, h2, h3 {{ margin: 18px 0 8px; page-break-after: avoid; }}
  ul.checklist {{ list-style: none; padding-left: 4px; }}
  ul.checklist li.done {{ color: #616061; text-decoration: line-through; }}
  pre {{
    background: #f6f6f6;
    border: 1px solid #e0e0e0;
    border-radius: 6px;
    padding: 12px;
    overflow-x: auto;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    break-inside: avoid;
  }}
  blockquote {{
    border-left: 3px solid #1264a3;
    margin: 12px 0;
    padding: 4px 12px;
    color: #4a4a4a;
    background: #f8f9fa;
  }}
  table {{ border-collapse: collapse; width: 100%; margin: 12px 0; break-inside: avoid; }}
  th, td {{ border: 1px solid #d8d8d8; padding: 6px 10px; text-align: left; }}
  th {{ background: #f2f2f2; font-weight: 600; }}
  figure {{ margin: 14px 0; break-inside: avoid; }}
  figure img {{ max-width: 100%; height: auto; border-radius: 6px; border: 1px solid #e0e0e0; }}
  figcaption {{ font-size: 12px; color: #616061; margin-top: 4px; }}
  .embed-card {{
    border: 1px solid #dcdcdc;
    border-radius: 6px;
    padding: 8px 12px;
    margin: 10px 0;
    background: #fafafa;
    break-inside: avoid;
  }}
  .mention {{
    background: #e8f5fa;
    color: #1264a3;
    border-radius: 4px;
    padding: 0 4px;
    font-weight: 500;
  }}
  .mention.broadcast {{ background: #fff3cd; color: #7a4f01; }}
  hr {{ border: none; border-top: 1px solid #ddd; margin: 18px 0; }}
  @media print {{
    body {{ margin: 0; max-width: none; padding: 0; }}
    @page {{ margin: 16mm; }}
  }}
</style>
</head>
<body>
{body}
</body>
</html>
"#
    )
}

/// Sanitize a canvas title into a safe filename stem.
pub fn canvas_export_stem(title: &str) -> String {
    let mut stem: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while stem.contains("--") {
        stem = stem.replace("--", "-");
    }
    let trimmed = stem.trim_matches('-');
    if trimmed.is_empty() {
        "canvas".to_owned()
    } else {
        trimmed.chars().take(48).collect()
    }
}

/// Write printable HTML for `doc` to `path` (pure filesystem helper, safe to test on temp dirs).
#[cfg(test)]
pub fn write_printable_html(doc: &CanvasDoc, path: &Path) -> std::io::Result<()> {
    let html = canvas_to_printable_html(doc);
    std::fs::write(path, html)
}

/// Open a print-ready HTML export in the OS print/browser flow (`F-COLLAB-182`).
/// Runs disk I/O and the `open` subprocess on a background thread so the GPUI UI thread never
/// blocks, and prunes stale temporary print HTML files to bound temp-directory growth.
/// Never spawns OS subprocesses or dialogs in `#[cfg(test)]`.
pub fn trigger_canvas_print(doc: &CanvasDoc) -> Result<PathBuf, String> {
    #[cfg(test)]
    {
        let _ = (
            doc,
            next_temp_html_path,
            TEMP_HTML_MAX_AGE,
            TEMP_HTML_MAX_FILES,
        );
        Err("OS print dialog disabled in test builds".to_owned())
    }
    #[cfg(not(test))]
    {
        let stem = canvas_export_stem(&doc.title);
        let path = next_temp_html_path("slackless-print", &stem);
        let html = canvas_to_printable_html(doc).replace(
            "</body>",
            "<script>window.addEventListener('load',function(){window.print();});</script>\n</body>",
        );
        let path_bg = path.clone();
        std::thread::Builder::new()
            .name("canvas-print".into())
            .spawn(move || {
                prune_stale_export_files(
                    &std::env::temp_dir(),
                    TEMP_HTML_MAX_AGE,
                    TEMP_HTML_MAX_FILES,
                );
                if let Err(err) = std::fs::write(&path_bg, html) {
                    tracing::warn!(path = %path_bg.display(), %err, "Failed to write print HTML");
                    return;
                }
                match std::process::Command::new("open").arg(&path_bg).status() {
                    Ok(status) if status.success() => {}
                    Ok(status) => {
                        tracing::warn!(%status, path = %path_bg.display(), "Print preview launch exited non-zero");
                    }
                    Err(err) => {
                        tracing::warn!(%err, path = %path_bg.display(), "Failed to launch print preview");
                    }
                }
            })
            .map_err(|e| format!("Failed to schedule print preview: {e}"))?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_pruning_keeps_the_newest_print_files_and_everything_else() {
        let dir = std::env::temp_dir().join(format!(
            "slackless-canvas-export-prune-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");

        // 5 print files pruned down to max_files=2; an unrelated file is never touched.
        for i in 0..5 {
            std::fs::write(dir.join(format!("slackless-print-doc-{i}.html")), "x")
                .expect("write print tmp");
        }
        let unrelated = dir.join("keep-me.html");
        std::fs::write(&unrelated, "keep").expect("write unrelated");

        prune_stale_export_files(&dir, Duration::from_secs(3600), 2);
        let remaining_tmp = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("slackless-print-"))
            })
            .count();
        assert_eq!(remaining_tmp, 2);
        assert!(unrelated.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
