//! Lenient deserializers for fields Slack sometimes sends as `null` or with a drifting type.

use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// `null` (or a missing field, with `#[serde(default)]`) becomes `T::default()`.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A field whose shape Slack varies: anything that does not parse as `T` becomes `None`
/// instead of failing the enclosing object.
pub(crate) fn ok_or_none<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = Value::deserialize(d)?;
    Ok(T::deserialize(&value).ok())
}

/// An integer that may arrive as a number, a numeric string, or `null`; anything else is `None`.
pub(crate) fn lenient_i64<'de, D>(d: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s
            .parse::<i64>()
            .ok()
            .or_else(|| s.parse::<f64>().ok().map(|f| f as i64)),
        _ => None,
    })
}
