//! Patchability macros: string enums and JSON-tagged enums that never fail on new values.

/// A string-valued enum (e.g. a message `subtype`) with an `Other(String)` catch-all, so a value
/// Slack adds tomorrow deserializes instead of erroring and serializes back verbatim.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $s:literal ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )*
            /// A value this crate does not model yet, kept verbatim.
            Other(String),
        }

        impl $name {
            /// The wire string.
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $s, )*
                    Self::Other(s) => s,
                }
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                match s {
                    $( $s => Self::$variant, )*
                    other => Self::Other(other.to_owned()),
                }
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                match Self::from(s.as_str()) {
                    Self::Other(_) => Self::Other(s),
                    known => known,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                String::deserialize(d).map(Self::from)
            }
        }
    };
}

/// An enum over a JSON object's tag field (`type` by default) with an
/// `Unknown { r#type, raw }` catch-all. Unknown tags, and known tags whose payload no longer
/// parses, land in `Unknown` with the raw JSON preserved: a Slack change is never fatal.
///
/// Payload structs do not carry the tag field; serialization re-inserts it.
macro_rules! tagged_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident (tag = $tag:literal, default = $default:expr) {
            $( $(#[$vmeta:meta])* $s:literal => $variant:ident($ty:ty) ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        pub enum $name {
            $( $(#[$vmeta])* $variant($ty), )*
            /// An unrecognized tag (or a payload that failed to parse), raw JSON preserved.
            Unknown {
                /// The tag string as received (empty when absent).
                r#type: String,
                /// The whole JSON object.
                raw: serde_json::Value,
            },
        }

        impl $name {
            /// Classifies a raw JSON object; never fails.
            pub fn from_value(raw: serde_json::Value) -> Self {
                let default_tag: Option<&str> = $default;
                let tag = raw
                    .get($tag)
                    .and_then(serde_json::Value::as_str)
                    .or(default_tag)
                    .unwrap_or_default()
                    .to_owned();
                match tag.as_str() {
                    $(
                        $s => {
                            if let Ok(v) = <$ty as serde::Deserialize>::deserialize(&raw) {
                                return Self::$variant(v);
                            }
                        }
                    )*
                    _ => {}
                }
                Self::Unknown { r#type: tag, raw }
            }

            /// The tag string of this value.
            pub fn type_str(&self) -> &str {
                match self {
                    $( Self::$variant(_) => $s, )*
                    Self::Unknown { r#type, .. } => r#type,
                }
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                serde_json::Value::deserialize(d).map(Self::from_value)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                match self {
                    $( Self::$variant(v) => $crate::macros::serialize_tagged($tag, $s, v, s), )*
                    Self::Unknown { raw, .. } => serde::Serialize::serialize(raw, s),
                }
            }
        }
    };
}

pub(crate) use string_enum;
pub(crate) use tagged_enum;

/// Serializes `value` as a JSON object with `tag_key: tag` inserted.
pub(crate) fn serialize_tagged<T, S>(
    tag_key: &str,
    tag: &str,
    value: &T,
    s: S,
) -> Result<S::Ok, S::Error>
where
    T: serde::Serialize,
    S: serde::Serializer,
{
    let mut json = serde_json::to_value(value).map_err(serde::ser::Error::custom)?;
    if let serde_json::Value::Object(map) = &mut json {
        map.insert(
            tag_key.to_owned(),
            serde_json::Value::String(tag.to_owned()),
        );
    }
    serde::Serialize::serialize(&json, s)
}
