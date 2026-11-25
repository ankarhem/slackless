//! Pure deep-link parsing: `slack://…` and `https://app.slack.com/client/…`.
//!
//! No OS calls here — the OS hand-off lives in [`crate::DeepLinkHandler`].
//! Patchability rule (ARCHITECTURE): unknown URL parameters are preserved in
//! [`SlackLink::extra`] and unmodeled `slack://` link kinds come back as
//! [`DeepLink::Other`]; parsing never drops data silently.

use std::collections::BTreeMap;

use percent_encoding::percent_decode_str;
use url::Url;

use crate::{PlatformError, Result};

/// A parsed deep link, structured per entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLink {
    /// `slack://…` — Slack's app deep links.
    Slack(SlackLink),
    /// `https://app.slack.com/client/<team>/<channel>[/<thread_ts>]`.
    WebClient(WebClientLink),
    /// `https://app.slack.com/huddle/<team>/<channel>` or `slack://huddle?team=<team>&id=<channel>`.
    Huddle {
        team_id: Option<String>,
        channel_id: String,
        raw: String,
    },
    /// `slack://app?team=<team>&id=<app>[&tab=home|messages|about]` (`F-COLLAB-628`, `F-COLLAB-629`).
    AppHome {
        team_id: Option<String>,
        app_id: String,
        tab: Option<String>,
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

/// The `slack://` link kinds Slack documents (channel, user, team, open).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackLinkKind {
    /// `slack://channel?id=<C…>&team=<T…>` — open a channel.
    Channel {
        channel_id: String,
        team_id: Option<String>,
    },
    /// `slack://user?id=<U…>&team=<T…>` — open a DM / user card.
    User {
        user_id: String,
        team_id: Option<String>,
    },
    /// `slack://team/<T…>` — switch to a workspace.
    Team { team_id: String },
    /// `slack://open?team=<T…>&id=<C…|U…|D…>` — open a conversation id.
    Open {
        team_id: Option<String>,
        target_id: Option<String>,
    },
}

/// An `https://app.slack.com/client/…` link (the web client mirrors the
/// desktop's `team`/`channel`/`thread_ts` path shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebClientLink {
    pub team_id: Option<String>,
    pub channel_id: Option<String>,
    /// From the third path segment or a `thread_ts` query parameter.
    pub thread_ts: Option<String>,
    /// The original URL string.
    pub raw: String,
}

/// Parse a URL into a [`DeepLink`].
///
/// Errors only when the input is not a URL at all, or is a URL this crate
/// explicitly does not handle (e.g. an unrelated https host). Recognized
/// schemes with unmodeled shapes come back as [`DeepLink::Other`].
pub fn parse_deep_link(url: &str) -> Result<DeepLink> {
    let parsed = Url::parse(url).map_err(|e| PlatformError::BadLink(format!("{url}: {e}")))?;
    match parsed.scheme() {
        "slack" => parse_slack(&parsed, url),
        "https" | "http" => parse_web_client(&parsed, url),
        _ => Err(PlatformError::BadLink(format!(
            "{url}: unsupported scheme {:?}",
            parsed.scheme()
        ))),
    }
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

    let kind = match host.as_str() {
        "channel" => SlackLinkKind::Channel {
            channel_id: param("id")
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    PlatformError::BadLink(format!("{raw}: channel link without ?id="))
                })?,
            team_id: param("team").map(str::to_string),
        },
        "user" => SlackLinkKind::User {
            user_id: param("id")
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .ok_or_else(|| PlatformError::BadLink(format!("{raw}: user link without ?id=")))?,
            team_id: param("team").map(str::to_string),
        },
        "team" => {
            let team_id = path
                .first()
                .cloned()
                .or_else(|| param("id").filter(|id| !id.is_empty()).map(str::to_string))
                .ok_or_else(|| {
                    PlatformError::BadLink(format!("{raw}: team link without a team id"))
                })?;
            SlackLinkKind::Team { team_id }
        }
        "open" => SlackLinkKind::Open {
            team_id: param("team").map(str::to_string),
            target_id: param("id").map(str::to_string),
        },
        "huddle" => {
            let channel_id = param("id")
                .or_else(|| param("channel"))
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    PlatformError::BadLink(format!("{raw}: huddle link without ?id="))
                })?;
            return Ok(DeepLink::Huddle {
                team_id: param("team").map(str::to_string),
                channel_id,
                raw: raw.to_string(),
            });
        }
        "app" => {
            let app_id = param("id")
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .ok_or_else(|| PlatformError::BadLink(format!("{raw}: app link without ?id=")))?;
            return Ok(DeepLink::AppHome {
                team_id: param("team").map(str::to_string),
                app_id,
                tab: param("tab")
                    .filter(|t| !t.is_empty())
                    .map(|t| t.to_ascii_lowercase()),
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
        SlackLinkKind::Channel { .. } | SlackLinkKind::User { .. } => &["id", "team"],
        SlackLinkKind::Team { .. } => &["id"],
        SlackLinkKind::Open { .. } => &["team", "id"],
    };
    let extra: BTreeMap<String, String> = pairs
        .into_iter()
        .filter(|(k, _)| !consumed.iter().any(|c| k.eq_ignore_ascii_case(c)))
        .collect();

    Ok(DeepLink::Slack(SlackLink {
        kind,
        extra,
        raw: raw.to_string(),
    }))
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
            let team_id = path.get(1).filter(|s| !s.is_empty()).cloned();
            let channel_id = path
                .get(2)
                .filter(|s| !s.is_empty())
                .cloned()
                .ok_or_else(|| {
                    PlatformError::BadLink(format!(
                        "{raw}: app.slack.com huddle link missing channel id"
                    ))
                })?;
            return Ok(DeepLink::Huddle {
                team_id,
                channel_id,
                raw: raw.to_string(),
            });
        }
        if path.first().map(String::as_str) != Some("client") {
            return Err(PlatformError::BadLink(format!(
                "{raw}: app.slack.com link outside /client/"
            )));
        }
        let thread_ts = path.get(3).cloned().or_else(|| {
            url.query_pairs()
                .find(|(k, _)| k == "thread_ts")
                .map(|(_, v)| v.into_owned())
        });
        return Ok(DeepLink::WebClient(WebClientLink {
            team_id: path.get(1).cloned(),
            channel_id: path.get(2).cloned(),
            thread_ts,
            raw: raw.to_string(),
        }));
    }

    // Workspace permalink: `https://<domain>.slack.com/archives/<C...>[/p<ts_digits>[?thread_ts=...&cid=...]]`
    if host.ends_with(".slack.com") && path.first().map(String::as_str) == Some("archives") {
        let cid_query = url
            .query_pairs()
            .find(|(k, _)| k == "cid")
            .map(|(_, v)| v.into_owned());
        let channel_id = cid_query.or_else(|| path.get(1).cloned());
        let thread_query = url
            .query_pairs()
            .find(|(k, _)| k == "thread_ts")
            .map(|(_, v)| v.into_owned());
        let p_ts = path.get(2).and_then(|seg| parse_archive_p_ts(seg));
        return Ok(DeepLink::WebClient(WebClientLink {
            team_id: None,
            channel_id,
            thread_ts: thread_query.or(p_ts),
            raw: raw.to_string(),
        }));
    }

    Err(PlatformError::BadLink(format!(
        "{raw}: https link is not a Slack client or archive URL"
    )))
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
            parse_deep_link(url).unwrap(),
            DeepLink::Slack(SlackLink {
                kind: SlackLinkKind::Channel {
                    channel_id: "C0123ABCDEF".into(),
                    team_id: Some("T0456GHIJKL".into()),
                },
                extra: BTreeMap::new(),
                raw: url.into(),
            })
        );
    }

    #[test]
    fn slack_channel_link_keeps_unknown_params() {
        let url = "slack://channel?id=C1&team=T2&message=1699999999.000100";
        match parse_deep_link(url).unwrap() {
            DeepLink::Slack(link) => {
                assert_eq!(
                    link.kind,
                    SlackLinkKind::Channel {
                        channel_id: "C1".into(),
                        team_id: Some("T2".into()),
                    }
                );
                assert_eq!(
                    link.extra.get("message").map(String::as_str),
                    Some("1699999999.000100")
                );
            }
            other => panic!("expected Slack link, got {other:?}"),
        }
    }

    #[test]
    fn slack_user_link() {
        let url = "slack://user?id=U0AAA111&team=T0BBB222";
        assert_eq!(
            parse_deep_link(url).unwrap(),
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
    fn slack_team_link_from_path() {
        let url = "slack://team/T0123456";
        match parse_deep_link(url).unwrap() {
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
        match parse_deep_link(url).unwrap() {
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
    }

    #[test]
    fn unmodeled_slack_link_preserved_verbatim() {
        let url = "slack://workflow/T0123456/bar";
        assert_eq!(
            parse_deep_link(url).unwrap(),
            DeepLink::Other { raw: url.into() }
        );
    }

    #[test]
    fn malformed_slack_channel_link_is_error() {
        // A channel link with no id cannot be acted on: it is an error.
        assert!(parse_deep_link("slack://channel?team=T1").is_err());
    }

    #[test]
    fn web_client_link() {
        let url = "https://app.slack.com/client/T0123ABC/C0456DEF";
        assert_eq!(
            parse_deep_link(url).unwrap(),
            DeepLink::WebClient(WebClientLink {
                team_id: Some("T0123ABC".into()),
                channel_id: Some("C0456DEF".into()),
                thread_ts: None,
                raw: url.into(),
            })
        );
    }

    #[test]
    fn web_client_link_with_thread_segment() {
        let url = "https://app.slack.com/client/T0123ABC/C0456DEF/1699999999.000100";
        match parse_deep_link(url).unwrap() {
            DeepLink::WebClient(link) => {
                assert_eq!(link.thread_ts.as_deref(), Some("1699999999.000100"));
            }
            other => panic!("expected WebClient link, got {other:?}"),
        }
    }

    #[test]
    fn web_client_link_with_thread_query_param() {
        let url = "https://app.slack.com/client/T0123ABC/C0456DEF?thread_ts=1699999999.000100";
        match parse_deep_link(url).unwrap() {
            DeepLink::WebClient(link) => {
                assert_eq!(link.thread_ts.as_deref(), Some("1699999999.000100"));
            }
            other => panic!("expected WebClient link, got {other:?}"),
        }
    }

    #[test]
    fn workspace_archive_permalink_parses_channel_and_ts() {
        let url = "https://norce.slack.com/archives/C0456DEF/p1700000001234567";
        assert_eq!(
            parse_deep_link(url).unwrap(),
            DeepLink::WebClient(WebClientLink {
                team_id: None,
                channel_id: Some("C0456DEF".into()),
                thread_ts: Some("1700000001.234567".into()),
                raw: url.into(),
            })
        );
        let thread_url = "https://norce.slack.com/archives/C0456DEF/p1700000001234567?thread_ts=1700000000.000100&cid=C0456DEF";
        match parse_deep_link(thread_url).unwrap() {
            DeepLink::WebClient(link) => {
                assert_eq!(link.channel_id.as_deref(), Some("C0456DEF"));
                assert_eq!(link.thread_ts.as_deref(), Some("1700000000.000100"));
            }
            other => panic!("expected WebClient link, got {other:?}"),
        }
    }

    #[test]
    fn non_client_web_link_is_error() {
        assert!(parse_deep_link("https://example.com/client/T1/C2").is_err());
        assert!(parse_deep_link("https://app.slack.com/docs").is_err());
    }

    #[test]
    fn garbage_is_error() {
        assert!(parse_deep_link("this is not a url").is_err());
        assert!(parse_deep_link("mailto:someone@example.com").is_err());
    }

    #[test]
    fn consumed_query_keys_are_stripped_case_insensitively() {
        let ch = parse_deep_link("slack://channel?ID=C1&TEAM=T2&Message=1.2").unwrap();
        match &ch {
            DeepLink::Slack(link) => {
                assert_eq!(
                    link.kind,
                    SlackLinkKind::Channel {
                        channel_id: "C1".into(),
                        team_id: Some("T2".into()),
                    }
                );
                assert_eq!(link.extra.len(), 1);
                assert_eq!(link.extra.get("Message").map(String::as_str), Some("1.2"));
            }
            other => panic!("expected Slack link, got {other:?}"),
        }
        assert_eq!(ch.raw(), "slack://channel?ID=C1&TEAM=T2&Message=1.2");

        let team = parse_deep_link("slack://team?ID=T1&ref=mail").unwrap();
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

        let open = parse_deep_link("slack://open?TEAM=T1&ID=C2").unwrap();
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
            parse_deep_link("slack://team?id=T1").unwrap(),
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
        .unwrap();
        match cid_override {
            DeepLink::WebClient(link) => {
                assert_eq!(link.channel_id.as_deref(), Some("C_OVERRIDE"));
                assert_eq!(link.thread_ts.as_deref(), Some("1700000001.234567"));
            }
            other => panic!("expected WebClient, got {other:?}"),
        }

        // Channel-only archive link (no message timestamp segment).
        let channel_only = parse_deep_link("https://acme.slack.com/archives/C0456DEF").unwrap();
        assert_eq!(
            channel_only,
            DeepLink::WebClient(WebClientLink {
                team_id: None,
                channel_id: Some("C0456DEF".into()),
                thread_ts: None,
                raw: "https://acme.slack.com/archives/C0456DEF".into(),
            })
        );

        let web_huddle =
            parse_deep_link("https://app.slack.com/huddle/T02HD8VA1/C02QYDW99ML").unwrap();
        assert_eq!(
            web_huddle,
            DeepLink::Huddle {
                team_id: Some("T02HD8VA1".into()),
                channel_id: "C02QYDW99ML".into(),
                raw: "https://app.slack.com/huddle/T02HD8VA1/C02QYDW99ML".into(),
            }
        );

        let slack_huddle = parse_deep_link("slack://huddle?team=T02HD8VA1&id=C02QYDW99ML").unwrap();
        assert_eq!(
            slack_huddle,
            DeepLink::Huddle {
                team_id: Some("T02HD8VA1".into()),
                channel_id: "C02QYDW99ML".into(),
                raw: "slack://huddle?team=T02HD8VA1&id=C02QYDW99ML".into(),
            }
        );

        let app_home = parse_deep_link("slack://app?team=T02HD8VA1&id=A01234567&tab=home").unwrap();
        assert_eq!(
            app_home,
            DeepLink::AppHome {
                team_id: Some("T02HD8VA1".into()),
                app_id: "A01234567".into(),
                tab: Some("home".into()),
                raw: "slack://app?team=T02HD8VA1&id=A01234567&tab=home".into(),
            }
        );
    }
}
