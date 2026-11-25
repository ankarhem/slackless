//! Patchability macros: string enums and JSON-tagged enums that never fail on new values.

use serde::ser::{self, Impossible, SerializeMap, SerializeStruct, Serializer};

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
                let tag_str = raw
                    .get($tag)
                    .and_then(serde_json::Value::as_str)
                    .or(default_tag)
                    .unwrap_or_default();
                match tag_str {
                    $(
                        $s => {
                            if let Ok(v) = <$ty as serde::Deserialize>::deserialize(&raw) {
                                return Self::$variant(v);
                            }
                        }
                    )*
                    _ => {}
                }
                let r#type = tag_str.to_owned();
                Self::Unknown { r#type, raw }
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

/// Serializes `value` as a JSON object with `tag_key: tag` inserted directly into `s`, avoiding
/// intermediate `serde_json::Value` tree allocations on nested tagged enums.
pub(crate) fn serialize_tagged<T, S>(
    tag_key: &'static str,
    tag: &'static str,
    value: &T,
    s: S,
) -> Result<S::Ok, S::Error>
where
    T: serde::Serialize,
    S: Serializer,
{
    value.serialize(InsertTagSerializer {
        inner: s,
        tag_key,
        tag,
    })
}

struct InsertTagSerializer<S> {
    inner: S,
    tag_key: &'static str,
    tag: &'static str,
}

impl<S: Serializer> Serializer for InsertTagSerializer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = S::SerializeSeq;
    type SerializeTuple = S::SerializeTuple;
    type SerializeTupleStruct = S::SerializeTupleStruct;
    type SerializeTupleVariant = S::SerializeTupleVariant;
    type SerializeMap = SkipKeyMap<S::SerializeMap>;
    type SerializeStruct = SkipFieldStruct<S::SerializeStruct>;
    type SerializeStructVariant = S::SerializeStructVariant;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_bool(v)
    }
    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i8(v)
    }
    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i16(v)
    }
    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i32(v)
    }
    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i64(v)
    }
    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u8(v)
    }
    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u16(v)
    }
    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u32(v)
    }
    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u64(v)
    }
    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_f32(v)
    }
    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_f64(v)
    }
    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_char(v)
    }
    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_str(v)
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_bytes(v)
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_none()
    }
    fn serialize_some<T: ?Sized + serde::Serialize>(
        self,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_some(value)
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_unit()
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_unit_struct(name)
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner
            .serialize_unit_variant(name, variant_index, variant)
    }
    fn serialize_newtype_struct<T: ?Sized + serde::Serialize>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_newtype_struct(name, value)
    }
    fn serialize_newtype_variant<T: ?Sized + serde::Serialize>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner
            .serialize_newtype_variant(name, variant_index, variant, value)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.inner.serialize_seq(len)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.inner.serialize_tuple(len)
    }
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.inner.serialize_tuple_struct(name, len)
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.inner
            .serialize_tuple_variant(name, variant_index, variant, len)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        let mut map = self.inner.serialize_map(len.map(|n| n + 1))?;
        map.serialize_entry(self.tag_key, self.tag)?;
        Ok(SkipKeyMap {
            inner: map,
            skip_key: self.tag_key,
            skip_next_value: false,
        })
    }
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        let mut st = self.inner.serialize_struct(name, len + 1)?;
        st.serialize_field(self.tag_key, self.tag)?;
        Ok(SkipFieldStruct {
            inner: st,
            skip_key: self.tag_key,
        })
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.inner
            .serialize_struct_variant(name, variant_index, variant, len)
    }
}

struct SkipFieldStruct<S> {
    inner: S,
    skip_key: &'static str,
}

impl<S: SerializeStruct> SerializeStruct for SkipFieldStruct<S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T: ?Sized + serde::Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        if key == self.skip_key {
            return Ok(());
        }
        self.inner.serialize_field(key, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.inner.end()
    }
}

struct SkipKeyMap<M> {
    inner: M,
    skip_key: &'static str,
    skip_next_value: bool,
}

impl<M: SerializeMap> SerializeMap for SkipKeyMap<M> {
    type Ok = M::Ok;
    type Error = M::Error;

    fn serialize_key<T: ?Sized + serde::Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        if key_equals_str(key, self.skip_key) {
            self.skip_next_value = true;
            return Ok(());
        }
        self.skip_next_value = false;
        self.inner.serialize_key(key)
    }

    fn serialize_value<T: ?Sized + serde::Serialize>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        if self.skip_next_value {
            self.skip_next_value = false;
            return Ok(());
        }
        self.inner.serialize_value(value)
    }

    fn serialize_entry<K: ?Sized + serde::Serialize, V: ?Sized + serde::Serialize>(
        &mut self,
        key: &K,
        value: &V,
    ) -> Result<(), Self::Error> {
        if key_equals_str(key, self.skip_key) {
            return Ok(());
        }
        self.inner.serialize_entry(key, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.inner.end()
    }
}

fn key_equals_str<K: ?Sized + serde::Serialize>(key: &K, expected: &str) -> bool {
    key.serialize(KeyEqualsStr(expected)).unwrap_or(false)
}

#[derive(Debug)]
struct NotMatching;

impl std::fmt::Display for NotMatching {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not matching")
    }
}

impl std::error::Error for NotMatching {}

impl ser::Error for NotMatching {
    fn custom<T: std::fmt::Display>(_msg: T) -> Self {
        Self
    }
}

struct KeyEqualsStr<'a>(&'a str);

impl Serializer for KeyEqualsStr<'_> {
    type Ok = bool;
    type Error = NotMatching;
    type SerializeSeq = Impossible<bool, NotMatching>;
    type SerializeTuple = Impossible<bool, NotMatching>;
    type SerializeTupleStruct = Impossible<bool, NotMatching>;
    type SerializeTupleVariant = Impossible<bool, NotMatching>;
    type SerializeMap = Impossible<bool, NotMatching>;
    type SerializeStruct = Impossible<bool, NotMatching>;
    type SerializeStructVariant = Impossible<bool, NotMatching>;

    fn serialize_str(self, v: &str) -> Result<bool, NotMatching> {
        Ok(v == self.0)
    }
    fn serialize_bool(self, _: bool) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_i8(self, _: i8) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_i16(self, _: i16) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_i32(self, _: i32) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_i64(self, _: i64) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_u8(self, _: u8) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_u16(self, _: u16) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_u32(self, _: u32) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_u64(self, _: u64) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_f32(self, _: f32) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_f64(self, _: f64) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_char(self, _: char) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_none(self) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_some<T: ?Sized + serde::Serialize>(self, _: &T) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_unit(self) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_newtype_struct<T: ?Sized + serde::Serialize>(
        self,
        _: &'static str,
        _: &T,
    ) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_newtype_variant<T: ?Sized + serde::Serialize>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<bool, NotMatching> {
        Ok(false)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, NotMatching> {
        Err(NotMatching)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, NotMatching> {
        Err(NotMatching)
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, NotMatching> {
        Err(NotMatching)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, NotMatching> {
        Err(NotMatching)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, NotMatching> {
        Err(NotMatching)
    }
    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, NotMatching> {
        Err(NotMatching)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, NotMatching> {
        Err(NotMatching)
    }
}
