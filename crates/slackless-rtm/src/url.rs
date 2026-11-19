//! Builds the message-server websocket URL the Slack web client dials.
//!
//! Parameter names live in [`param`] and the host in [`slackless_types::endpoints::WSS_PRIMARY`],
//! so a renamed flag or moved host is a one-line edit. Observed values: docs/research/api-realtime.md
//! ("URL, full parameter set").

use slackless_types::TeamId;
use slackless_types::endpoints::WSS_PRIMARY;
use url::Url;

/// Query parameter names on `wss://wss-primary.slack.com/`.
pub mod param {
    /// User session token (`xoxc-...`).
    pub const TOKEN: &str = "token";
    /// Sync flag; semantics unknown, always `1` in the web client.
    pub const SYNC_DESYNC: &str = "sync_desync";
    /// Client identity string (`desktop`).
    pub const SLACK_CLIENT: &str = "slack_client";
    /// Nested, URL-encoded query of boot-agent options (see [`super::ConnectParams::start_args`]).
    pub const START_ARGS: &str = "start_args";
    /// Do not re-query on channel subscribe.
    pub const NO_QUERY_ON_SUBSCRIBE: &str = "no_query_on_subscribe";
    /// Flannel protocol version (`3`).
    pub const FLANNEL: &str = "flannel";
    /// Defer channel hydration to `client.counts` / the HTTP API.
    pub const LAZY_CHANNELS: &str = "lazy_channels";
    /// Message-server shard, `<team_id>-1`.
    pub const GATEWAY_SERVER: &str = "gateway_server";
    /// `presence_change` arrives batched (`users[]`); required for `presence_sub`.
    pub const BATCH_PRESENCE_AWARE: &str = "batch_presence_aware";
}

/// Inputs to [`connect_url`]. [`ConnectParams::new`] fills in the web client's observed defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectParams {
    pub token: String,
    /// `gateway_server`; omitted when `None`. Use [`ConnectParams::with_team`] to derive it.
    pub gateway_server: Option<String>,
    pub slack_client: Option<String>,
    pub sync_desync: bool,
    pub no_query_on_subscribe: bool,
    pub flannel: Option<u32>,
    pub lazy_channels: bool,
    pub batch_presence_aware: bool,
    /// Key/value pairs encoded as `?k=v&...` into the single `start_args` param; empty omits it.
    pub start_args: Vec<(String, String)>,
    /// Extra params appended verbatim, for flags Slack adds later.
    pub extra: Vec<(String, String)>,
}

/// `start_args` observed from the web client (slackdump FlannelURL, slackgo).
pub const DEFAULT_START_ARGS: &[(&str, &str)] = &[
    ("agent", "client"),
    ("org_wide_aware", "true"),
    ("eac_cache_ts", "true"),
    ("cache_ts", "0"),
    ("name_tagging", "true"),
    ("only_self_subteams", "true"),
    ("connect_only", "true"),
    ("ms_latest", "true"),
];

impl ConnectParams {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            gateway_server: None,
            slack_client: Some("desktop".to_owned()),
            sync_desync: true,
            no_query_on_subscribe: true,
            flannel: Some(3),
            lazy_channels: true,
            batch_presence_aware: true,
            start_args: DEFAULT_START_ARGS
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            extra: Vec::new(),
        }
    }

    /// Sets `gateway_server=<team_id>-1` (use the enterprise id for org-level teams).
    pub fn with_team(mut self, team: &TeamId) -> Self {
        self.gateway_server = Some(format!("{team}-1"));
        self
    }
}

/// Builds the connect URL on [`WSS_PRIMARY`].
pub fn connect_url(params: &ConnectParams) -> Url {
    connect_url_on(WSS_PRIMARY, params).expect("WSS_PRIMARY is a valid URL")
}

/// Builds the connect URL on an arbitrary base (e.g. a test server or a moved host).
pub fn connect_url_on(base: &str, p: &ConnectParams) -> Result<Url, url::ParseError> {
    let mut url = Url::parse(base)?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair(param::TOKEN, &p.token);
        if p.sync_desync {
            q.append_pair(param::SYNC_DESYNC, "1");
        }
        if let Some(client) = &p.slack_client {
            q.append_pair(param::SLACK_CLIENT, client);
        }
        if !p.start_args.is_empty() {
            let nested = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(&p.start_args)
                .finish();
            q.append_pair(param::START_ARGS, &format!("?{nested}"));
        }
        if p.no_query_on_subscribe {
            q.append_pair(param::NO_QUERY_ON_SUBSCRIBE, "1");
        }
        if let Some(v) = p.flannel {
            q.append_pair(param::FLANNEL, &v.to_string());
        }
        if p.lazy_channels {
            q.append_pair(param::LAZY_CHANNELS, "1");
        }
        if let Some(gw) = &p.gateway_server {
            q.append_pair(param::GATEWAY_SERVER, gw);
        }
        if p.batch_presence_aware {
            q.append_pair(param::BATCH_PRESENCE_AWARE, "1");
        }
        q.extend_pairs(&p.extra);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn pairs(url: &Url) -> HashMap<String, String> {
        url.query_pairs().into_owned().collect()
    }

    #[test]
    fn default_url_carries_token_and_web_client_params() {
        let p = ConnectParams::new("xoxc-123-abc").with_team(&TeamId::new("T0FC8BFQR"));
        let url = connect_url(&p);
        assert_eq!(url.scheme(), "wss");
        assert_eq!(url.host_str(), Some("wss-primary.slack.com"));

        let q = pairs(&url);
        assert_eq!(q["token"], "xoxc-123-abc");
        assert_eq!(q["sync_desync"], "1");
        assert_eq!(q["slack_client"], "desktop");
        assert_eq!(q["no_query_on_subscribe"], "1");
        assert_eq!(q["flannel"], "3");
        assert_eq!(q["lazy_channels"], "1");
        assert_eq!(q["gateway_server"], "T0FC8BFQR-1");
        assert_eq!(q["batch_presence_aware"], "1");
        assert_eq!(
            q["start_args"],
            "?agent=client&org_wide_aware=true&eac_cache_ts=true&cache_ts=0\
             &name_tagging=true&only_self_subteams=true&connect_only=true&ms_latest=true"
        );
        // The nested query is percent-encoded on the wire, as the web client sends it.
        assert!(
            url.as_str()
                .contains("start_args=%3Fagent%3Dclient%26org_wide_aware%3Dtrue")
        );
    }

    #[test]
    fn optional_params_can_be_dropped_or_extended() {
        let mut p = ConnectParams::new("xoxc-1");
        p.flannel = None;
        p.lazy_channels = false;
        p.start_args.clear();
        p.extra.push(("new_flag".into(), "x y".into()));
        let q = pairs(&connect_url_on("wss://example.test/", &p).unwrap());
        assert_eq!(q["token"], "xoxc-1");
        assert_eq!(q["new_flag"], "x y");
        for gone in ["flannel", "lazy_channels", "start_args", "gateway_server"] {
            assert!(!q.contains_key(gone), "{gone} should be omitted");
        }
    }
}
