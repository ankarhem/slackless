//! Pure deep-link parsing: `slack://…`, `https://app.slack.com/client/…`, and
//! `https://<workspace>.slack.com/archives/…`.
//!
//! Patchability rule (ARCHITECTURE): unknown URL parameters are preserved in
//! `extra` and unmodeled `slack://` link kinds come back as
//! [`DeepLink::Other`]; parsing never drops data silently.

use std::collections::BTreeMap;

use percent_encoding::percent_decode_str;
use slackless_types::net::is_trusted_slack_host;
use url::Url;

use crate::{PlatformError, Result};

/// A parsed deep link, structured per entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLink {
    /// `slack://…` — Slack's app deep links.
    Slack(SlackLink),
    /// `https://app.slack.com/client/…` or `https://<workspace>.slack.com/archives/…`.
    WebClient(WebClientLink),
    /// `https://app.slack.com/huddle/<team>/<channel>` or `slack://huddle?team=<team>&id=<channel>`.
    Huddle {
        team_id: Option<String>,
        channel_id: String,
        /// Query parameters beyond the modeled ones (never dropped).
        extra: BTreeMap<String, String>,
        raw: String,
    },
    /// `slack://app?team=<team>&id=<app>[&tab=home|messages|about]` (`F-COLLAB-628`, `F-COLLAB-629`).
    AppHome {
        team_id: Option<String>,
        app_id: String,
        tab: Option<String>,
        /// Query parameters beyond the modeled ones (never dropped).
        extra: BTreeMap<String, String>,
        raw: String,
    },
    /// A `slack://` link kind we do not model yet, preserved verbatim.
    Other { raw: String },
}

impl DeepLink {
    /// The original URL string this link was parsed from.
    pub fn raw(&self) -> &str {
        match self {
            DeepLink::Slack(link) => &link.raw,
            DeepLink::WebClient(link) => &link.raw,
            DeepLink::Huddle { raw, .. } => raw,
            DeepLink::AppHome { raw, .. } => raw,
            DeepLink::Other { raw } => raw,
        }
    }
}

/// A `slack://…` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackLink {
    /// What the link points at.
    pub kind: SlackLinkKind,
    /// Query parameters beyond the ones modeled in `kind` (never dropped).
    pub extra: BTreeMap<String, String>,
    /// The original URL string.
    pub raw: String,
}

/// The `slack://` link kinds Slack documents (`channel`, `user`, `team`, `open`, `file`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackLinkKind {
    /// `slack://channel?team=<T…>&id=<C…>[&message=<ts>][&thread_ts=<ts>]` — open a channel,
    /// message, or thread.
    Channel {
        channel_id: String,
        team_id: Option<String>,
        /// Target message timestamp (normalized from `message=` or `p<digits>`).
        message_ts: Option<String>,
        /// Target thread root timestamp (`thread_ts=`).
        thread_ts: Option<String>,
    },
    /// `slack://user?team=<T…>&id=<U…>` — open a DM / user card.
    User {
        user_id: String,
        team_id: Option<String>,
    },
    /// `slack://team/<T…>` or `slack://team?id=<T…>` — switch to a workspace.
    Team { team_id: String },
    /// `slack://open?team=<T…>[&id=<C…|U…|D…|F…>]` — open a workspace or target conversation/user/file.
    Open {
        team_id: Option<String>,
        target_id: Option<String>,
    },
    /// `slack://file?team=<T…>&id=<F…>` — open a shared file in the target workspace.
    File {
        file_id: String,
        team_id: Option<String>,
    },
}

/// An `https://app.slack.com/client/…` or `https://<ws>.slack.com/archives/…` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebClientLink {
    pub team_id: Option<String>,
    /// Workspace subdomain from `https://<ws>.slack.com/archives/…` (lowercase, e.g. `"norce"`).
    pub workspace_domain: Option<String>,
    pub channel_id: Option<String>,
    /// Exact message timestamp (e.g. from `/archives/<ch>/p<digits>`).
    pub message_ts: Option<String>,
    /// Thread root timestamp (from `?thread_ts=…`, `/client/<team>/<ch>/<ts>`, or
    /// `/client/<team>/<ch>/thread/<ch>-<ts>`).
    pub thread_ts: Option<String>,
    /// The original URL string.
    pub raw: String,
}

/// Parse a URL into a [`DeepLink`].
///
/// Errors only when the input is not a URL at all, has malformed required fields, or is an
/// unrelated https host. Recognized `slack://` schemes with unmodeled hosts come back as
/// [`DeepLink::Other`].
pub fn parse_deep_link(url: &str) -> Result<DeepLink> {
    let trimmed = url.trim();
    let parsed = Url::parse(trimmed).map_err(|e| PlatformError::BadLink(format!("{url}: {e}")))?;
    match parsed.scheme() {
        "slack" => parse_slack(&parsed, trimmed),
        "https" => parse_web_client(&parsed, trimmed),
        _ => Err(PlatformError::BadLink(format!(
            "{url}: unsupported scheme {:?}",
            parsed.scheme()
        ))),
    }
}

/// Reject empty, control-character, whitespace, or path-separator Slack IDs.
fn sanitize_id(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty()
        || s.len() > 128
        || s.bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace() || b == b'/' || b == b'\\')
    {
        return None;
    }
    Some(s.to_owned())
}

/// Normalize a Slack message or thread timestamp from either dotted (`1700000001.234567`) or
/// permalink (`p1700000001234567`) form. Rejects malformed/non-numeric strings.
pub(crate) fn normalize_slack_ts(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() || s.len() > 40 {
        return None;
    }
    if s.starts_with('p') || s.starts_with('P') {
        let lower = format!("p{}", &s[1..]);
        return parse_archive_p_ts(&lower);
    }
    if let Some((secs, frac)) = s.split_once('.') {
        if !secs.is_empty()
            && !frac.is_empty()
            && secs.bytes().all(|b| b.is_ascii_digit())
            && frac.bytes().all(|b| b.is_ascii_digit())
        {
            return Some(format!("{secs}.{frac}"));
        }
        return None;
    }
    if s.len() > 6 && s.bytes().all(|b| b.is_ascii_digit()) {
        let split = s.len() - 6;
        return Some(format!("{}.{}", &s[..split], &s[split..]));
    }
    None
}

fn parse_slack(url: &Url, raw: &str) -> Result<DeepLink> {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let path: Vec<String> = url
        .path()
        .split('/')
        .filter(|seg| !seg.is_empty())
        .map(decode_segment)
        .collect();

    let param = |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    };
    let id_param = |key: &str| param(key).and_then(sanitize_id);
    let ts_param = |key: &str| param(key).and_then(normalize_slack_ts);

    let kind = match host.as_str() {
        "channel" => SlackLinkKind::Channel {
            channel_id: id_param("id").ok_or_else(|| {
                PlatformError::BadLink(format!("{raw}: channel link without valid ?id="))
            })?,
            team_id: id_param("team"),
            message_ts: ts_param("message"),
            thread_ts: ts_param("thread_ts"),
        },
        "user" => SlackLinkKind::User {
            user_id: id_param("id").ok_or_else(|| {
                PlatformError::BadLink(format!("{raw}: user link without valid ?id="))
            })?,
            team_id: id_param("team"),
        },
        "team" => {
            let team_id = path
                .first()
                .and_then(|s| sanitize_id(s))
                .or_else(|| id_param("id"))
                .or_else(|| id_param("team"))
                .ok_or_else(|| {
                    PlatformError::BadLink(format!("{raw}: team link without a valid team id"))
                })?;
            SlackLinkKind::Team { team_id }
        }
        "open" => SlackLinkKind::Open {
            team_id: id_param("team"),
            target_id: id_param("id"),
        },
        "file" => {
            let file_id = id_param("id")
                .or_else(|| match path.as_slice() {
                    [only] => sanitize_id(only),
                    [_team, file, ..] => sanitize_id(file),
                    [] => None,
                })
                .ok_or_else(|| {
                    PlatformError::BadLink(format!("{raw}: file link without valid ?id="))
                })?;
            let team_id = id_param("team").or_else(|| match path.as_slice() {
                [team, _file, ..] => sanitize_id(team),
                _ => None,
            });
            SlackLinkKind::File { file_id, team_id }
        }
        "huddle" => {
            let channel_id = id_param("id")
                .or_else(|| id_param("channel"))
                .ok_or_else(|| {
                    PlatformError::BadLink(format!("{raw}: huddle link without valid ?id="))
                })?;
            let team_id = id_param("team");
            let extra = collect_extra(pairs, &["id", "channel", "team"]);
            return Ok(DeepLink::Huddle {
                team_id,
                channel_id,
                extra,
                raw: raw.to_string(),
            });
        }
        "app" => {
            let app_id = id_param("id").ok_or_else(|| {
                PlatformError::BadLink(format!("{raw}: app link without valid ?id="))
            })?;
            let team_id = id_param("team");
            let tab = param("tab")
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| t.to_ascii_lowercase());
            let extra = collect_extra(pairs, &["id", "team", "tab"]);
            return Ok(DeepLink::AppHome {
                team_id,
                app_id,
                tab,
                extra,
                raw: raw.to_string(),
            });
        }
        // Unmodeled slack:// kind: keep it verbatim (patchable, not fatal).
        _ => {
            return Ok(DeepLink::Other {
                raw: raw.to_string(),
            });
        }
    };

    // Params the chosen kind consumed; everything else is preserved verbatim.
    let consumed: &[&str] = match &kind {
        SlackLinkKind::Channel { .. } => &["id", "team", "message", "thread_ts"],
        SlackLinkKind::User { .. } | SlackLinkKind::File { .. } => &["id", "team"],
        SlackLinkKind::Team { .. } => &["id", "team"],
        SlackLinkKind::Open { .. } => &["team", "id"],
    };
    let extra = collect_extra(pairs, consumed);

    Ok(DeepLink::Slack(SlackLink {
        kind,
        extra,
        raw: raw.to_string(),
    }))
}

fn collect_extra(pairs: Vec<(String, String)>, consumed: &[&str]) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .filter(|(k, _)| !consumed.iter().any(|c| k.eq_ignore_ascii_case(c)))
        .collect()
}

fn query_param_ci(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.into_owned())
}

fn parse_web_client(url: &Url, raw: &str) -> Result<DeepLink> {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let path: Vec<String> = url
        .path()
        .split('/')
        .filter(|seg| !seg.is_empty())
        .map(decode_segment)
        .collect();

    if host == "app.slack.com" {
        if path.first().map(String::as_str) == Some("huddle") {
            let team_id = path.get(1).and_then(|s| sanitize_id(s));
            let channel_id = path.get(2).and_then(|s| sanitize_id(s)).ok_or_else(|| {
                PlatformError::BadLink(format!(
                    "{raw}: app.slack.com huddle link missing channel id"
                ))
            })?;
            let extra: BTreeMap<String, String> = url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            return Ok(DeepLink::Huddle {
                team_id,
                channel_id,
                extra,
                raw: raw.to_string(),
            });
        }
        if path.first().map(String::as_str) != Some("client") {
            return Err(PlatformError::BadLink(format!(
                "{raw}: app.slack.com link outside /client/"
            )));
        }
        let team_id = path.get(1).and_then(|s| sanitize_id(s));
        let channel_id = path.get(2).and_then(|s| sanitize_id(s));
        let query_thread_ts = query_param_ci(url, "thread_ts").and_then(|v| normalize_slack_ts(&v));
        let query_message_ts = query_param_ci(url, "message").and_then(|v| normalize_slack_ts(&v));

        // Handle `/client/<team>/<channel>/thread/<channel>-<ts>` AND `/client/<team>/<channel>/<ts>`.
        let path_thread_ts = match (
            path.get(3).map(String::as_str),
            path.get(4).map(String::as_str),
        ) {
            (Some("thread"), Some(thread_seg)) => thread_seg
                .split_once('-')
                .map(|(_, ts)| ts)
                .unwrap_or(thread_seg)
                .trim()
                .pipe_normalize_ts(),
            (Some(seg), _) if !seg.eq_ignore_ascii_case("thread") => normalize_slack_ts(seg),
            _ => None,
        };
        let thread_ts = query_thread_ts.or(path_thread_ts);
        return Ok(DeepLink::WebClient(WebClientLink {
            team_id,
            workspace_domain: None,
            channel_id,
            message_ts: query_message_ts,
            thread_ts,
            raw: raw.to_string(),
        }));
    }

    // Workspace permalink: `https://<domain>.slack.com/archives/<C...>[/p<ts_digits>[?thread_ts=...&cid=...]]`
    if is_trusted_slack_host(&host)
        && host != "slack.com"
        && path.first().map(String::as_str) == Some("archives")
    {
        let workspace_domain = host
            .strip_suffix(".slack.com")
            .filter(|sub| !sub.is_empty())
            .map(str::to_owned);
        let team_id = query_param_ci(url, "team").and_then(|v| sanitize_id(&v));
        let cid_query = query_param_ci(url, "cid").and_then(|v| sanitize_id(&v));
        let channel_id = cid_query.or_else(|| path.get(1).and_then(|s| sanitize_id(s)));
        if channel_id.is_none() {
            return Err(PlatformError::BadLink(format!(
                "{raw}: archive link missing channel id"
            )));
        }
        let thread_ts = query_param_ci(url, "thread_ts").and_then(|v| normalize_slack_ts(&v));
        let message_ts = path
            .get(2)
            .and_then(|seg| parse_archive_p_ts(seg))
            .or_else(|| query_param_ci(url, "message").and_then(|v| normalize_slack_ts(&v)));
        return Ok(DeepLink::WebClient(WebClientLink {
            team_id,
            workspace_domain,
            channel_id,
            message_ts,
            thread_ts,
            raw: raw.to_string(),
        }));
    }

    Err(PlatformError::BadLink(format!(
        "{raw}: https link is not a Slack client or archive URL"
    )))
}

trait StrTsExt {
    fn pipe_normalize_ts(self) -> Option<String>;
}

impl StrTsExt for &str {
    fn pipe_normalize_ts(self) -> Option<String> {
        normalize_slack_ts(self)
    }
}

/// Convert a Slack permalink `p1700000000123456` segment into `1700000000.123456`.
fn parse_archive_p_ts(seg: &str) -> Option<String> {
    let digits = seg.strip_prefix('p')?;
    if digits.len() > 6 && digits.bytes().all(|b| b.is_ascii_digit()) {
        let split = digits.len() - 6;
        Some(format!("{}.{}", &digits[..split], &digits[split..]))
    } else {
        None
    }
}

/// Percent-decode one path segment, keeping the raw text if it is not UTF-8.
fn decode_segment(seg: &str) -> String {
    percent_decode_str(seg)
        .decode_utf8()
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| seg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_channel_link() {
        let url = "slack://channel?id=C0123ABCDEF&team=T0456GHIJKL";
        assert_eq!(
            parse_deep_link(url).expect("parse channel link"),
            DeepLink::Slack(SlackLink {
                kind: SlackLinkKind::Channel {
                    channel_id: "C0123ABCDEF".into(),
                    team_id: Some("T0456GHIJKL".into()),
                    message_ts: None,
                    thread_ts: None,
                },
                extra: BTreeMap::new(),
                raw: url.into(),
            })
        );
    }

    #[test]
    fn slack_channel_link_parses_message_and_thread_ts_and_keeps_unknown_params() {
        let url = "slack://channel?id=C1&team=T2&message=1699999999.000100&thread_ts=1699999900.000050&ref=notif";
        match parse_deep_link(url).expect("parse channel link with message & thread") {
            DeepLink::Slack(link) => {
                assert_eq!(
                    link.kind,
                    SlackLinkKind::Channel {
                        channel_id: "C1".into(),
                        team_id: Some("T2".into()),
                        message_ts: Some("1699999999.000100".into()),
                        thread_ts: Some("1699999900.000050".into()),
                    }
                );
                assert_eq!(link.extra.get("ref").map(String::as_str), Some("notif"));
                assert!(!link.extra.contains_key("message"));
                assert!(!link.extra.contains_key("thread_ts"));
            }
            other => panic!("expected Slack link, got {other:?}"),
        }

        // Also supports `p<digits>` message format and rejects empty team gracefully.
        let p_url = "slack://channel?team=&id=C1&message=p1700000001234567";
        match parse_deep_link(p_url).expect("parse p-ts channel link") {
            DeepLink::Slack(link) => {
                assert_eq!(
                    link.kind,
                    SlackLinkKind::Channel {
                        channel_id: "C1".into(),
                        team_id: None,
                        message_ts: Some("1700000001.234567".into()),
                        thread_ts: None,
                    }
                );
            }
            other => panic!("expected Slack link, got {other:?}"),
        }
    }

    #[test]
    fn slack_user_link() {
        let url = "slack://user?id=U0AAA111&team=T0BBB222";
        assert_eq!(
            parse_deep_link(url).expect("parse user link"),
            DeepLink::Slack(SlackLink {
                kind: SlackLinkKind::User {
                    user_id: "U0AAA111".into(),
                    team_id: Some("T0BBB222".into()),
                },
                extra: BTreeMap::new(),
                raw: url.into(),
            })
        );
    }

    #[test]
    fn slack_file_link_query_and_path_variants() {
        let url = "slack://file?team=T0BBB222&id=F0999XYZ&ref=email";
        assert_eq!(
            parse_deep_link(url).expect("parse file query link"),
            DeepLink::Slack(SlackLink {
                kind: SlackLinkKind::File {
                    file_id: "F0999XYZ".into(),
                    team_id: Some("T0BBB222".into()),
                },
                extra: BTreeMap::from([("ref".into(), "email".into())]),
                raw: url.into(),
            })
        );

        let path_url = "slack://file/T0BBB222/F0999XYZ";
        assert_eq!(
            parse_deep_link(path_url).expect("parse file path link"),
            DeepLink::Slack(SlackLink {
                kind: SlackLinkKind::File {
                    file_id: "F0999XYZ".into(),
                    team_id: Some("T0BBB222".into()),
                },
                extra: BTreeMap::new(),
                raw: path_url.into(),
            })
        );

        assert!(parse_deep_link("slack://file?team=T1").is_err());
        assert!(parse_deep_link("slack://file?id=").is_err());
    }

    #[test]
    fn slack_team_link_from_path() {
        let url = "slack://team/T0123456";
        match parse_deep_link(url).expect("parse team link") {
            DeepLink::Slack(link) => {
                assert_eq!(
                    link.kind,
                    SlackLinkKind::Team {
                        team_id: "T0123456".into()
                    }
                );
                assert!(link.extra.is_empty());
            }
            other => panic!("expected Slack link, got {other:?}"),
        }
    }

    #[test]
    fn slack_open_link() {
        let url = "slack://open?team=T0123456&id=C0666777";
        match parse_deep_link(url).expect("parse open link") {
            DeepLink::Slack(link) => {
                assert_eq!(
                    link.kind,
                    SlackLinkKind::Open {
                        team_id: Some("T0123456".into()),
                        target_id: Some("C0666777".into()),
                    }
                );
            }
            other => panic!("expected Slack link, got {other:?}"),
        }

        // Team-only open link and empty-value filtering.
        let team_only = parse_deep_link("slack://open?team=T0123456&id=").expect("team-only open");
        assert_eq!(
            team_only,
            DeepLink::Slack(SlackLink {
                kind: SlackLinkKind::Open {
                    team_id: Some("T0123456".into()),
                    target_id: None,
                },
                extra: BTreeMap::new(),
                raw: "slack://open?team=T0123456&id=".into(),
            })
        );
    }

    #[test]
    fn unmodeled_slack_link_preserved_verbatim() {
        let url = "slack://workflow/T0123456/bar";
        assert_eq!(
            parse_deep_link(url).expect("parse other slack link"),
            DeepLink::Other { raw: url.into() }
        );
    }

    #[test]
    fn malformed_slack_channel_link_is_error() {
        assert!(parse_deep_link("slack://channel?team=T1").is_err());
        assert!(parse_deep_link("slack://channel?team=T1&id=%20%20").is_err());
        assert!(parse_deep_link("slack://channel?team=T1&id=../etc").is_err());
    }

    #[test]
    fn web_client_link() {
        let url = "https://app.slack.com/client/T0123ABC/C0456DEF";
        assert_eq!(
            parse_deep_link(url).expect("parse web client link"),
            DeepLink::WebClient(WebClientLink {
                team_id: Some("T0123ABC".into()),
                workspace_domain: None,
                channel_id: Some("C0456DEF".into()),
                message_ts: None,
                thread_ts: None,
                raw: url.into(),
            })
        );
    }

    #[test]
    fn web_client_link_with_thread_segment_and_thread_path() {
        let url = "https://app.slack.com/client/T0123ABC/C0456DEF/1699999999.000100";
        match parse_deep_link(url).expect("parse web client thread segment") {
            DeepLink::WebClient(link) => {
                assert_eq!(link.thread_ts.as_deref(), Some("1699999999.000100"));
            }
            other => panic!("expected WebClient link, got {other:?}"),
        }

        // `/client/<team>/<channel>/thread/<channel>-<ts>` parses the real ts, never literal "thread".
        let thread_path =
            "https://app.slack.com/client/T0123ABC/C0456DEF/thread/C0456DEF-1700000001.000100";
        match parse_deep_link(thread_path).expect("parse web client thread path") {
            DeepLink::WebClient(link) => {
                assert_eq!(link.channel_id.as_deref(), Some("C0456DEF"));
                assert_eq!(link.thread_ts.as_deref(), Some("1700000001.000100"));
            }
            other => panic!("expected WebClient link, got {other:?}"),
        }

        // Explicit `?thread_ts=` takes precedence over path segment.
        let query_wins = "https://app.slack.com/client/T0123ABC/C0456DEF/thread/C0456DEF-1700000001.000100?thread_ts=1700000002.000200";
        match parse_deep_link(query_wins).expect("parse web client thread query override") {
            DeepLink::WebClient(link) => {
                assert_eq!(link.thread_ts.as_deref(), Some("1700000002.000200"));
            }
            other => panic!("expected WebClient link, got {other:?}"),
        }
    }

    #[test]
    fn web_client_link_with_thread_query_param() {
        let url = "https://app.slack.com/client/T0123ABC/C0456DEF?thread_ts=1699999999.000100";
        match parse_deep_link(url).expect("parse web client thread query") {
            DeepLink::WebClient(link) => {
                assert_eq!(link.thread_ts.as_deref(), Some("1699999999.000100"));
            }
            other => panic!("expected WebClient link, got {other:?}"),
        }
    }

    #[test]
    fn workspace_archive_permalink_preserves_domain_channel_message_and_thread_ts() {
        let url = "https://norce.slack.com/archives/C0456DEF/p1700000001234567";
        assert_eq!(
            parse_deep_link(url).expect("parse archive permalink"),
            DeepLink::WebClient(WebClientLink {
                team_id: None,
                workspace_domain: Some("norce".into()),
                channel_id: Some("C0456DEF".into()),
                message_ts: Some("1700000001.234567".into()),
                thread_ts: None,
                raw: url.into(),
            })
        );
        let thread_url = "https://norce.slack.com/archives/C0456DEF/p1700000001234567?thread_ts=1700000000.000100&cid=C0456DEF";
        match parse_deep_link(thread_url).expect("parse archive thread reply permalink") {
            DeepLink::WebClient(link) => {
                assert_eq!(link.workspace_domain.as_deref(), Some("norce"));
                assert_eq!(link.channel_id.as_deref(), Some("C0456DEF"));
                assert_eq!(link.message_ts.as_deref(), Some("1700000001.234567"));
                assert_eq!(link.thread_ts.as_deref(), Some("1700000000.000100"));
            }
            other => panic!("expected WebClient link, got {other:?}"),
        }
    }

    #[test]
    fn non_client_and_untrusted_web_links_are_rejected() {
        for bad in [
            "https://example.com/client/T1/C2",
            "https://app.slack.com/docs",
            "https://evil.slack.com.attacker.com/archives/C1/p1700000001234567",
            "https://slack.com/archives/C1/p1700000001234567",
            "https://acme.slack.com/archives/",
        ] {
            assert!(
                parse_deep_link(bad).is_err(),
                "expected BadLink error for {bad}"
            );
        }
    }

    #[test]
    fn garbage_is_error() {
        assert!(parse_deep_link("this is not a url").is_err());
        assert!(parse_deep_link("mailto:someone@example.com").is_err());
    }

    #[test]
    fn consumed_query_keys_are_stripped_case_insensitively() {
        let ch = parse_deep_link("slack://channel?ID=C1&TEAM=T2&Message=1.2&ref=banner")
            .expect("parse case-insensitive channel link");
        match &ch {
            DeepLink::Slack(link) => {
                assert_eq!(
                    link.kind,
                    SlackLinkKind::Channel {
                        channel_id: "C1".into(),
                        team_id: Some("T2".into()),
                        message_ts: Some("1.2".into()),
                        thread_ts: None,
                    }
                );
                assert_eq!(link.extra.len(), 1);
                assert_eq!(link.extra.get("ref").map(String::as_str), Some("banner"));
            }
            other => panic!("expected Slack link, got {other:?}"),
        }
        assert_eq!(
            ch.raw(),
            "slack://channel?ID=C1&TEAM=T2&Message=1.2&ref=banner"
        );

        let team = parse_deep_link("slack://team?ID=T1&ref=mail").expect("parse team query");
        match team {
            DeepLink::Slack(link) => {
                assert_eq!(
                    link.kind,
                    SlackLinkKind::Team {
                        team_id: "T1".into()
                    }
                );
                assert_eq!(link.extra.len(), 1);
                assert_eq!(link.extra.get("ref").map(String::as_str), Some("mail"));
            }
            other => panic!("expected Slack link, got {other:?}"),
        }

        let open = parse_deep_link("slack://open?TEAM=T1&ID=C2").expect("parse open query");
        match open {
            DeepLink::Slack(link) => {
                assert!(link.extra.is_empty());
                assert_eq!(
                    link.kind,
                    SlackLinkKind::Open {
                        team_id: Some("T1".into()),
                        target_id: Some("C2".into()),
                    }
                );
            }
            other => panic!("expected Slack link, got {other:?}"),
        }
    }

    #[test]
    fn team_query_fallback_and_empty_or_missing_id_errors() {
        assert_eq!(
            parse_deep_link("slack://team?id=T1").expect("parse team query fallback"),
            DeepLink::Slack(SlackLink {
                kind: SlackLinkKind::Team {
                    team_id: "T1".into()
                },
                extra: BTreeMap::new(),
                raw: "slack://team?id=T1".into(),
            })
        );

        for bad in [
            "slack://channel?id=",
            "slack://user",
            "slack://user?id=",
            "slack://team",
            "slack://team?id=",
            "slack://file",
            "slack://file?id=",
        ] {
            assert!(
                parse_deep_link(bad).is_err(),
                "expected BadLink error for {bad}"
            );
        }
    }

    #[test]
    fn decode_segment_and_archive_edge_cases() {
        assert_eq!(decode_segment("hello%20world"), "hello world");
        assert_eq!(decode_segment("%C3%A5"), "å");
        // Invalid UTF-8 bytes fall back to the raw segment text.
        assert_eq!(decode_segment("%FF%FE"), "%FF%FE");

        // parse_archive_p_ts boundary lengths (<=6 digits invalid, 7 digits valid).
        assert_eq!(parse_archive_p_ts("p123456"), None);
        assert_eq!(parse_archive_p_ts("p1234567"), Some("1.234567".to_owned()));
        assert_eq!(parse_archive_p_ts("p123456a"), None);
        assert_eq!(parse_archive_p_ts("1234567"), None);

        // `cid` query overrides the path channel id.
        let cid_override = parse_deep_link(
            "https://acme.slack.com/archives/C_PATH/p1700000001234567?cid=C_OVERRIDE",
        )
        .expect("parse cid override");
        match cid_override {
            DeepLink::WebClient(link) => {
                assert_eq!(link.workspace_domain.as_deref(), Some("acme"));
                assert_eq!(link.channel_id.as_deref(), Some("C_OVERRIDE"));
                assert_eq!(link.message_ts.as_deref(), Some("1700000001.234567"));
                assert_eq!(link.thread_ts, None);
            }
            other => panic!("expected WebClient, got {other:?}"),
        }

        // Channel-only archive link (no message timestamp segment).
        let channel_only = parse_deep_link("https://acme.slack.com/archives/C0456DEF")
            .expect("parse channel-only archive");
        assert_eq!(
            channel_only,
            DeepLink::WebClient(WebClientLink {
                team_id: None,
                workspace_domain: Some("acme".into()),
                channel_id: Some("C0456DEF".into()),
                message_ts: None,
                thread_ts: None,
                raw: "https://acme.slack.com/archives/C0456DEF".into(),
            })
        );

        let web_huddle = parse_deep_link("https://app.slack.com/huddle/T02HD8VA1/C02QYDW99ML")
            .expect("parse web huddle");
        assert_eq!(
            web_huddle,
            DeepLink::Huddle {
                team_id: Some("T02HD8VA1".into()),
                channel_id: "C02QYDW99ML".into(),
                extra: BTreeMap::new(),
                raw: "https://app.slack.com/huddle/T02HD8VA1/C02QYDW99ML".into(),
            }
        );

        let slack_huddle =
            parse_deep_link("slack://huddle?team=T02HD8VA1&id=C02QYDW99ML&source=invite")
                .expect("parse slack huddle");
        assert_eq!(
            slack_huddle,
            DeepLink::Huddle {
                team_id: Some("T02HD8VA1".into()),
                channel_id: "C02QYDW99ML".into(),
                extra: BTreeMap::from([("source".into(), "invite".into())]),
                raw: "slack://huddle?team=T02HD8VA1&id=C02QYDW99ML&source=invite".into(),
            }
        );

        let app_home =
            parse_deep_link("slack://app?team=T02HD8VA1&id=A01234567&tab=home&ref=banner")
                .expect("parse app home");
        assert_eq!(
            app_home,
            DeepLink::AppHome {
                team_id: Some("T02HD8VA1".into()),
                app_id: "A01234567".into(),
                tab: Some("home".into()),
                extra: BTreeMap::from([("ref".into(), "banner".into())]),
                raw: "slack://app?team=T02HD8VA1&id=A01234567&tab=home&ref=banner".into(),
            }
        );
    }

    #[test]
    fn rejects_cleartext_http_links() {
        for link in [
            "http://app.slack.com/client/T0123ABC/C0456DEF",
            "http://app.slack.com/huddle/T0123ABC/C0456DEF",
            "http://acme.slack.com/archives/C0456DEF/p1700000001234567",
        ] {
            assert!(
                parse_deep_link(link).is_err(),
                "expected cleartext http link to be rejected: {link}"
            );
        }
    }

    #[test]
    fn enterprise_grid_archive_permalink_preserves_workspace_domain() {
        let link =
            parse_deep_link("https://acme.enterprise.slack.com/archives/C01ENG/p1700000001234567")
                .expect("parse grid archive link");
        match link {
            DeepLink::WebClient(web) => {
                assert_eq!(web.workspace_domain.as_deref(), Some("acme.enterprise"));
                assert_eq!(web.channel_id.as_deref(), Some("C01ENG"));
                assert_eq!(web.message_ts.as_deref(), Some("1700000001.234567"));
            }
            other => panic!("expected WebClient link, got {other:?}"),
        }
    }
}
