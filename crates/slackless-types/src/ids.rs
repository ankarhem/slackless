//! Id and timestamp newtypes. All serialize as plain JSON strings.

use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(id: &str) -> Self {
                Self(id.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(id: String) -> Self {
                Self(id)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    };
}

id_newtype!(
    /// A conversation id (channel `C…`, private `G…`, DM `D…`).
    ChannelId
);
id_newtype!(
    /// A user id (`U…` / `W…`).
    UserId
);
id_newtype!(
    /// A workspace (`T…`) or enterprise (`E…`) id.
    TeamId
);
id_newtype!(
    /// A file id (`F…`).
    FileId
);
id_newtype!(
    /// A bot id (`B…`).
    BotId
);
id_newtype!(
    /// A user group id (`S…`).
    UsergroupId
);

/// A Slack message timestamp (`"1700000000.000100"`); also a message's id within a channel.
///
/// Ordering is numeric (seconds, then fractional digits), so differently padded timestamps sort
/// correctly. Deserializes from a string or, leniently, a number.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Ts(pub String);

impl Ts {
    pub fn new(ts: impl Into<String>) -> Self {
        Self(ts.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whole seconds since the Unix epoch, if the value parses.
    pub fn secs(&self) -> Option<i64> {
        self.split().0.parse().ok()
    }

    fn split(&self) -> (&str, &str) {
        self.0.split_once('.').unwrap_or((&self.0, ""))
    }
}

impl Ord for Ts {
    fn cmp(&self, other: &Self) -> Ordering {
        let (a_secs, a_frac) = self.split();
        let (b_secs, b_frac) = other.split();
        let secs = match (a_secs.parse::<u64>(), b_secs.parse::<u64>()) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            _ => a_secs.cmp(b_secs),
        };
        // Comparing fractional digit strings lexicographically, trailing zeros stripped, is
        // decimal-fraction order. The raw-string tiebreak keeps Ord consistent with Eq.
        secs.then_with(|| {
            a_frac
                .trim_end_matches('0')
                .cmp(b_frac.trim_end_matches('0'))
        })
        .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for Ts {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Ts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Ts {
    fn from(ts: &str) -> Self {
        Self(ts.to_owned())
    }
}

impl From<String> for Ts {
    fn from(ts: String) -> Self {
        Self(ts)
    }
}

impl AsRef<str> for Ts {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Ts {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = Ts;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a Slack timestamp string or number")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Ts, E> {
                Ok(Ts(v.to_owned()))
            }

            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Ts, E> {
                Ok(Ts(v))
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Ts, E> {
                Ok(Ts(v.to_string()))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Ts, E> {
                Ok(Ts(v.to_string()))
            }

            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Ts, E> {
                Ok(Ts(format!("{v:.6}")))
            }
        }

        d.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_orders_numerically_across_padding() {
        assert!(Ts::from("1700000000.000100") < Ts::from("1700000000.0002"));
        assert!(Ts::from("999999999.9") < Ts::from("1700000000.000001"));
        assert_eq!(
            Ts::from("1700000000.0001").cmp(&Ts::from("1700000000.000100")),
            Ordering::Less // equal value, raw-string tiebreak
        );
        assert_eq!(Ts::from("1700000000.000100").secs(), Some(1_700_000_000));
    }

    #[test]
    fn ids_are_plain_strings_on_the_wire() {
        let ts: Ts = serde_json::from_str("1700000000").unwrap();
        assert_eq!(ts, Ts::from("1700000000"));
        assert_eq!(
            serde_json::to_string(&ChannelId::from("C1")).unwrap(),
            "\"C1\""
        );
        assert_eq!(serde_json::to_string(&Ts::from("1.2")).unwrap(), "\"1.2\"");
    }
}
