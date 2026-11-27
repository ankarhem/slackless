//! Shared Slack networking helpers: cookie encoding, URL redaction, host trust checks, and
//! canonical huddle fallback URLs. Pure string/URL operations; no I/O.

use crate::endpoints::WEB_ORIGIN;

/// URL-encodes the `d` cookie value exactly once. Browsers store it already encoded
/// (`xoxd-...%2F...%3D`); double-encoding breaks auth, so a value made only of URL-safe
/// characters and `%` escapes is kept as is.
pub fn encode_cookie(value: &str) -> String {
    let already_encoded = value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-._~%".contains(&b));
    if already_encoded {
        value.to_owned()
    } else {
        url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
    }
}

/// Strips query parameters, fragments, and embedded userinfo/passwords from `raw` so signed Slack
/// tokens, tickets, or credentials never land in logs.
pub fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        Err(_) => "<invalid-url>".to_owned(),
    }
}

/// Whether `host` belongs to Slack (`slack.com` or any valid subdomain such as `app.slack.com`,
/// `acme.enterprise.slack.com`, or cookie domain `.slack.com`).
///
/// Rejects lookalike hosts (`evilslack.com`, `slack.com.evil.example`) and malformed empty labels
/// (`..slack.com`, `a..slack.com`).
pub fn is_trusted_slack_host(host: &str) -> bool {
    let normalized = host.strip_prefix('.').unwrap_or(host);
    if normalized.is_empty() || normalized.split('.').any(str::is_empty) {
        return false;
    }
    if normalized.eq_ignore_ascii_case("slack.com") {
        return true;
    }
    let suffix = ".slack.com";
    normalized.len() > suffix.len()
        && normalized.as_bytes()[normalized.len() - suffix.len()..]
            .eq_ignore_ascii_case(suffix.as_bytes())
}

/// Canonical web-client huddle join URL when `rooms.getLink` does not return one.
pub fn huddle_fallback_url(workspace: &str, channel: &str) -> String {
    format!("{WEB_ORIGIN}/huddle/{workspace}/{channel}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_cookie_encodes_raw_values_once_and_preserves_encoded() {
        assert_eq!(
            encode_cookie("xoxd-abc/def+ghi=="),
            "xoxd-abc%2Fdef%2Bghi%3D%3D"
        );
        // Already percent-encoded values are left untouched (no double encoding).
        assert_eq!(
            encode_cookie("xoxd-abc%2Fdef%2Bghi%3D%3D"),
            "xoxd-abc%2Fdef%2Bghi%3D%3D"
        );
        assert_eq!(encode_cookie("xoxd-plain_123.~-"), "xoxd-plain_123.~-");
        // Spaces and control characters are encoded, so a `HeaderValue` never rejects the cookie.
        assert_eq!(encode_cookie("xoxd-a b/c\r\n"), "xoxd-a+b%2Fc%0D%0A");
    }

    #[test]
    fn redact_url_strips_query_fragment_and_userinfo() {
        assert_eq!(
            redact_url("https://user:secret@wss-primary.slack.com/link?token=xoxc-123#frag"),
            "https://wss-primary.slack.com/link"
        );
        assert_eq!(
            redact_url("wss://wss-primary.slack.com/?ticket=secret&version=5"),
            "wss://wss-primary.slack.com/"
        );
        assert_eq!(
            redact_url("https://onlyuser@acme.slack.com/api/auth.test?t=1"),
            "https://acme.slack.com/api/auth.test"
        );
        assert_eq!(redact_url("not a valid url"), "<invalid-url>");
    }

    #[test]
    fn is_trusted_slack_host_accepts_slack_domains_and_rejects_lookalikes() {
        for ok in [
            "slack.com",
            ".slack.com",
            "SLACK.COM",
            "app.slack.com",
            "APP.SLACK.COM",
            "acme.enterprise.slack.com",
            ".app.slack.com",
        ] {
            assert!(is_trusted_slack_host(ok), "expected trusted: {ok:?}");
        }

        for bad in [
            "",
            ".",
            "..slack.com",
            "a..slack.com",
            "slack.com.",
            "evilslack.com",
            "slack.com.evil.example",
            "example.com",
            "not-slack.com",
            "ééééééa",
            "a.éééééa",
            "😀😀😀.slack.com.evil",
        ] {
            assert!(!is_trusted_slack_host(bad), "expected untrusted: {bad:?}");
        }
        assert!(is_trusted_slack_host("é.slack.com"));
    }

    #[test]
    fn huddle_fallback_url_builds_canonical_web_link() {
        assert_eq!(
            huddle_fallback_url("T0123ABC", "C0456DEF"),
            "https://app.slack.com/huddle/T0123ABC/C0456DEF"
        );
    }
}
