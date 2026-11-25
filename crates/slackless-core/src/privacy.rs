//! Client-side privacy helpers: URL tracking-parameter stripping and pre-send secret detection.

use url::Url;

/// One potential credential or private key detected in outgoing message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMatch {
    /// Human-readable category (e.g. `"Slack token"`, `"AWS access key"`).
    pub kind: &'static str,
    /// Redacted short preview safe to display in a warning dialog or CLI error.
    pub preview: String,
}

const TRACKING_PARAMS: &[&str] = &[
    "fbclid",
    "gclid",
    "gbraid",
    "wbraid",
    "dclid",
    "msclkid",
    "mc_cid",
    "mc_eid",
    "igshid",
    "si",
    "ref_src",
    "ref_url",
    "_hsenc",
    "_hsmi",
    "mkt_tok",
    "yclid",
    "oly_anon_id",
    "oly_enc_id",
    "vero_id",
    "s_cid",
];

fn is_tracking_param(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.starts_with("utm_") || TRACKING_PARAMS.contains(&lower.as_str())
}

/// Remove known analytics/tracking query parameters (`utm_*`, `fbclid`, `gclid`, `si`, etc.)
/// from an `http://` or `https://` URL while preserving legitimate query parameters and
/// fragments. Non-HTTP or unparseable strings are returned unchanged.
pub fn strip_tracking_params(raw: &str) -> String {
    let Ok(mut parsed) = Url::parse(raw) else {
        return raw.to_owned();
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return raw.to_owned();
    }
    if parsed.query().is_none() {
        return raw.to_owned();
    }
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let orig_count = parsed.query_pairs().count();
    if kept.len() == orig_count {
        return raw.to_owned();
    }
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        let mut pairs = parsed.query_pairs_mut();
        pairs.clear();
        for (k, v) in kept {
            pairs.append_pair(&k, &v);
        }
    }
    parsed.into()
}

/// Strip tracking parameters from every `http://` or `https://` URL embedded in `text`.
pub fn strip_tracking_params_in_text(text: &str) -> String {
    if !text.contains("http://") && !text.contains("https://") {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest
        .find("http://")
        .into_iter()
        .chain(rest.find("https://"))
        .min()
    {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '|' | '"' | '\'' | ')'))
            .unwrap_or(after.len());
        let candidate = &after[..end];
        // Keep trailing sentence punctuation (`.` / `,` / `;`) outside the URL.
        let trimmed = candidate.trim_end_matches(['.', ',', ';']);
        let trailing = &candidate[trimmed.len()..];
        out.push_str(&strip_tracking_params(trimmed));
        out.push_str(trailing);
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn mask_secret(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 10 {
        return format!("{}…", chars.iter().take(4).collect::<String>());
    }
    let head: String = chars.iter().take(6).collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("{head}…{tail}")
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+' | b'/' | b'=')
}

fn token_span_from(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && is_token_char(bytes[end]) {
        end += 1;
    }
    end
}

fn is_boundary_before(bytes: &[u8], idx: usize) -> bool {
    idx == 0 || matches!(bytes[idx - 1], b'=' | b':' | b'/') || !is_token_char(bytes[idx - 1])
}

fn push_unique(out: &mut Vec<SecretMatch>, kind: &'static str, token: &str) {
    let preview = mask_secret(token);
    if !out.iter().any(|m| m.kind == kind && m.preview == preview) {
        out.push(SecretMatch { kind, preview });
    }
}

fn for_each_token_with_prefix(
    text: &str,
    bytes: &[u8],
    prefix: &str,
    mut on_token: impl FnMut(&str),
) {
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(prefix) {
        let start = search_from + rel;
        let end = token_span_from(bytes, start);
        if is_boundary_before(bytes, start) {
            on_token(&text[start..end]);
        }
        search_from = start + prefix.len();
    }
}

/// Scan outgoing message text for common leaked credentials, API keys, JWTs, private keys, and
/// database connection strings with embedded passwords.
pub fn detect_secrets(text: &str) -> Vec<SecretMatch> {
    let mut hits = Vec::new();

    // 1. PEM private key headers.
    for marker in [
        "-----BEGIN PRIVATE KEY-----",
        "-----BEGIN RSA PRIVATE KEY-----",
        "-----BEGIN EC PRIVATE KEY-----",
        "-----BEGIN OPENSSH PRIVATE KEY-----",
        "-----BEGIN DSA PRIVATE KEY-----",
        "-----BEGIN PGP PRIVATE KEY BLOCK-----",
    ] {
        if text.contains(marker) {
            push_unique(&mut hits, "PEM private key", marker);
        }
    }

    let bytes = text.as_bytes();

    // 2. Known high-signal token prefixes.
    const PREFIX_RULES: &[(&str, &str, usize)] = &[
        ("xoxb-", "Slack bot token", 20),
        ("xoxp-", "Slack user token", 20),
        ("xoxc-", "Slack session token", 20),
        ("xoxd-", "Slack session cookie", 20),
        ("xoxa-", "Slack app token", 20),
        ("xoxr-", "Slack refresh token", 20),
        ("xapp-", "Slack app-level token", 20),
        ("ghp_", "GitHub personal access token", 24),
        ("gho_", "GitHub OAuth token", 24),
        ("ghs_", "GitHub server token", 24),
        ("github_pat_", "GitHub fine-grained token", 30),
        ("glpat-", "GitLab personal access token", 20),
        ("sk-ant-", "Anthropic API key", 28),
        ("sk-proj-", "OpenAI API key", 28),
        ("sk_live_", "Stripe live secret key", 24),
        ("rk_live_", "Stripe restricted key", 24),
        ("AIza", "Google API key", 35),
    ];

    for &(prefix, kind, min_len) in PREFIX_RULES {
        for_each_token_with_prefix(text, bytes, prefix, |tok| {
            if tok.len() >= min_len {
                push_unique(&mut hits, kind, tok);
            }
        });
    }

    // OpenAI legacy `sk-` keys (exclude `sk-ant-` / `sk-proj-` already matched above).
    for_each_token_with_prefix(text, bytes, "sk-", |tok| {
        if !tok.starts_with("sk-ant-")
            && !tok.starts_with("sk-proj-")
            && tok.len() >= 32
            && tok[3..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            push_unique(&mut hits, "API secret key (sk-...)", tok);
        }
    });

    // 3. AWS Access Key IDs (`AKIA` + 16 uppercase alphanumeric chars).
    for_each_token_with_prefix(text, bytes, "AKIA", |tok| {
        if tok.len() == 20
            && tok[4..]
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            push_unique(&mut hits, "AWS access key ID", tok);
        }
    });

    // 4. JWTs (`eyJ<header>.eyJ<payload>.<signature>`).
    for_each_token_with_prefix(text, bytes, "eyJ", |tok| {
        let parts: Vec<&str> = tok.split('.').collect();
        if parts.len() == 3
            && parts[0].len() >= 10
            && parts[1].starts_with("eyJ")
            && parts[1].len() >= 10
            && parts[2].len() >= 16
        {
            push_unique(&mut hits, "JSON Web Token (JWT)", tok);
        }
    });

    // 5. Database / cache URLs with embedded passwords (`scheme://user:pass@host`).
    for scheme in [
        "postgres://",
        "postgresql://",
        "mysql://",
        "mongodb://",
        "mongodb+srv://",
        "redis://",
        "rediss://",
        "amqp://",
        "amqps://",
    ] {
        let mut search_from = 0;
        while let Some(rel) = text[search_from..].find(scheme) {
            let start = search_from + rel;
            let rest = &text[start..];
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | ')'))
                .unwrap_or(rest.len());
            let candidate = &rest[..end];
            if let Ok(parsed) = Url::parse(candidate)
                && parsed.password().is_some_and(|p| !p.is_empty())
            {
                push_unique(&mut hits, "Database URL with password", candidate);
            }
            search_from = start + scheme.len();
        }
    }

    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_tracking_params_removes_trackers_and_preserves_real_params() {
        assert_eq!(
            strip_tracking_params(
                "https://example.com/post?id=42&utm_source=twitter&utm_medium=social&fbclid=abc#top"
            ),
            "https://example.com/post?id=42#top"
        );
        assert_eq!(
            strip_tracking_params("https://youtu.be/dQw4w9WgXcQ?si=track123"),
            "https://youtu.be/dQw4w9WgXcQ"
        );
        assert_eq!(
            strip_tracking_params("https://norce.io/docs?page=2&sort=asc"),
            "https://norce.io/docs?page=2&sort=asc"
        );
        assert_eq!(
            strip_tracking_params("slack://channel?team=T1&id=C1"),
            "slack://channel?team=T1&id=C1"
        );
        assert_eq!(
            strip_tracking_params_in_text(
                "Check https://example.com/a?utm_campaign=q4&x=1 and https://example.com/b?fbclid=99."
            ),
            "Check https://example.com/a?x=1 and https://example.com/b."
        );
    }

    #[test]
    fn detect_secrets_catches_real_patterns_and_avoids_false_positives() {
        let slack_tok = format!("xoxb-{}-{}", "123456789012", "abcdefghijklmnopqrstuvwx");
        let aws_key = format!("AKIA{}", "IOSFODNN7EXAMPLE");
        let gh_tok = format!("ghp_{}", "1234567890abcdef1234567890abcdef1234");
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let db = "postgres://admin:supersecret@db.internal:5432/prod";
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjE=";

        assert_eq!(detect_secrets(&slack_tok)[0].kind, "Slack bot token");
        assert!(
            !detect_secrets(&slack_tok)[0]
                .preview
                .contains("123456789012")
        );
        assert_eq!(detect_secrets(&aws_key)[0].kind, "AWS access key ID");
        assert_eq!(
            detect_secrets(&gh_tok)[0].kind,
            "GitHub personal access token"
        );
        assert_eq!(detect_secrets(jwt)[0].kind, "JSON Web Token (JWT)");
        assert_eq!(detect_secrets(db)[0].kind, "Database URL with password");
        assert_eq!(detect_secrets(pem)[0].kind, "PEM private key");

        // Normal messages (English + Swedish + clean URLs + UUIDs) must not trigger.
        for benign in [
            "Hej! Kan vi ta ett möte kl 14?",
            "Deploy finished for commit 66ec38d8e5a826263b38f63562aa03a2d9b07f14",
            "https://postgres.org/docs/16/index.html",
            "postgres://readonly@localhost:5432/dev",
            "Look at sk-short or AKIA123 (too short)",
        ] {
            assert!(
                detect_secrets(benign).is_empty(),
                "false positive on {benign:?}: {:?}",
                detect_secrets(benign)
            );
        }
    }
}
