//! Lenient deserializers for response fields whose shape Slack varies.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// `null`, a missing field (with `#[serde(default)]`), or any value that does not parse as `T`
/// becomes `T::default()` instead of failing the enclosing response.
pub(crate) fn or_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = Value::deserialize(d)?;
    Ok(T::deserialize(&value).unwrap_or_default())
}

/// `null` (or a missing field, with `#[serde(default)]`) becomes `T::default()`; any other
/// shape mismatch is still an error.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A directory-style list (channels, users, counts): entries that do not parse are skipped so
/// one odd object never empties the whole list; a non-array becomes empty.
pub(crate) fn lenient_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(match Value::deserialize(d)? {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| T::deserialize(v).ok())
            .collect(),
        _ => Vec::new(),
    })
}

/// A count that may arrive as a number, a numeric string, or `null`.
pub(crate) fn lenient_u32<'de, D>(d: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
        Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    })
}

/// A unix timestamp / 64-bit count that may arrive as an integer, numeric string, or `null`.
pub(crate) fn lenient_u64<'de, D>(d: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
            .unwrap_or(0),
        Value::String(s) => s
            .trim()
            .split('.')
            .next()
            .and_then(|whole| whole.parse().ok())
            .unwrap_or(0),
        _ => 0,
    })
}

/// A flag that may arrive as a bool, `0`/`1`, `"0"`/`"1"`, or `null`.
pub(crate) fn lenient_bool<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(d)? {
        Value::Bool(b) => b,
        Value::Number(n) => n.as_i64().is_some_and(|n| n != 0),
        Value::String(s) => matches!(s.as_str(), "1" | "true"),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Default, PartialEq, Deserialize)]
    struct Item {
        id: u32,
    }

    #[derive(Debug, Default, PartialEq, Deserialize)]
    struct Wire {
        #[serde(default, deserialize_with = "lenient_bool")]
        flag: bool,
        #[serde(default, deserialize_with = "lenient_u32")]
        count: u32,
        #[serde(default, deserialize_with = "lenient_vec")]
        items: Vec<Item>,
        #[serde(default, deserialize_with = "or_default")]
        fallback: u32,
        #[serde(default, deserialize_with = "null_default")]
        name: String,
    }

    #[test]
    fn lenient_bool_accepts_bools_ints_and_strings() {
        let f = |v: Value| {
            serde_json::from_value::<Wire>(json!({ "flag": v }))
                .unwrap()
                .flag
        };
        assert!(f(json!(true)));
        assert!(!f(json!(false)));
        assert!(f(json!(1)));
        assert!(f(json!(-2)));
        assert!(!f(json!(0)));
        assert!(f(json!("1")));
        assert!(f(json!("true")));
        assert!(!f(json!("0")));
        assert!(!f(json!("false")));
        assert!(!f(json!("yes")));
        assert!(!f(json!(null)));
        assert!(!f(json!([])));
    }

    #[test]
    fn lenient_u32_parses_numbers_and_strings_and_zeroes_invalid() {
        let c = |v: Value| {
            serde_json::from_value::<Wire>(json!({ "count": v }))
                .unwrap()
                .count
        };
        assert_eq!(c(json!(42)), 42);
        assert_eq!(c(json!("99")), 99);
        assert_eq!(c(json!(u64::from(u32::MAX) + 1)), 0);
        assert_eq!(c(json!(-1)), 0);
        assert_eq!(c(json!("abc")), 0);
        assert_eq!(c(json!(null)), 0);
        assert_eq!(c(json!({})), 0);
    }

    #[test]
    fn lenient_vec_skips_malformed_elements_and_handles_non_array() {
        let w: Wire = serde_json::from_value(json!({
            "items": [
                { "id": 1 },
                { "bad": true },
                "not-an-object",
                { "id": 2 }
            ]
        }))
        .unwrap();
        assert_eq!(w.items, vec![Item { id: 1 }, Item { id: 2 }]);

        let non_arr: Wire = serde_json::from_value(json!({ "items": "oops" })).unwrap();
        assert!(non_arr.items.is_empty());

        let null_arr: Wire = serde_json::from_value(json!({ "items": null })).unwrap();
        assert!(null_arr.items.is_empty());
    }

    #[test]
    fn or_default_and_null_default_behave_as_documented() {
        let w: Wire = serde_json::from_value(json!({
            "fallback": "wrong-type",
            "name": null
        }))
        .unwrap();
        assert_eq!(w.fallback, 0);
        assert_eq!(w.name, "");
        assert!(serde_json::from_value::<Wire>(json!({ "name": 123 })).is_err());
    }
}
