//! Lenient deserializers and serde helpers for fields Slack sometimes sends as `null` or with a
//! drifting type.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Serde `skip_serializing_if` predicate for omitting `false` booleans on the wire.
pub fn is_false(b: &bool) -> bool {
    !*b
}

/// `null` (or a missing field, with `#[serde(default)]`) becomes `T::default()`; any other
/// shape mismatch is still an error.
pub fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// `null`, a missing field (with `#[serde(default)]`), or any value that does not parse as `T`
/// becomes `T::default()` instead of failing the enclosing object.
pub fn or_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = Value::deserialize(d)?;
    Ok(T::deserialize(&value).unwrap_or_default())
}

/// A field whose shape Slack varies: anything that does not parse as `T` becomes `None`
/// instead of failing the enclosing object.
pub fn ok_or_none<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(d)?;
    Ok(T::deserialize(&value).ok())
}

/// A directory-style list (channels, users, counts): entries that do not parse are skipped so
/// one odd object never empties the whole list; a non-array becomes empty.
pub fn lenient_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
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

/// An integer that may arrive as a number, a numeric string, or `null`; anything else (including
/// non-finite floats) is `None`.
pub fn lenient_i64<'de, D>(d: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            trimmed.parse::<i64>().ok().or_else(|| {
                trimmed
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .map(|f| f as i64)
            })
        }
        _ => None,
    })
}

/// A unix timestamp / 64-bit unsigned count that may arrive as a number, numeric string (including
/// fractional Slack timestamps like `"1700000000.000100"`), or `null`. Negative, non-finite, or
/// unparseable values become `0`.
pub fn lenient_u64<'de, D>(d: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| {
                n.as_f64()
                    .filter(|f| f.is_finite() && *f >= 0.0)
                    .map(|f| f as u64)
            })
            .unwrap_or(0),
        Value::String(s) => {
            let trimmed = s.trim();
            if let Ok(v) = trimmed.parse::<u64>() {
                v
            } else if let Some((whole, frac)) = trimmed.split_once('.')
                && !frac.is_empty()
                && frac.bytes().all(|b| b.is_ascii_digit())
                && let Ok(v) = whole.parse::<u64>()
            {
                v
            } else {
                trimmed
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite() && *f >= 0.0)
                    .map(|f| f as u64)
                    .unwrap_or(0)
            }
        }
        _ => 0,
    })
}

/// A 32-bit unsigned count that may arrive as a number, numeric string, or `null`. Negative,
/// overflowing, non-finite, or unparseable values become `0`.
pub fn lenient_u32<'de, D>(d: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .or_else(|| {
                n.as_f64()
                    .filter(|f| f.is_finite() && *f >= 0.0 && *f <= f64::from(u32::MAX))
                    .map(|f| f as u32)
            })
            .unwrap_or(0),
        Value::String(s) => {
            let trimmed = s.trim();
            trimmed.parse::<u32>().ok().unwrap_or(0)
        }
        _ => 0,
    })
}

/// A flag that may arrive as a bool, non-zero number, `"1"` / `"true"`, or `null`.
pub fn lenient_bool<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(d)? {
        Value::Bool(b) => b,
        Value::Number(n) => {
            n.as_i64().is_some_and(|v| v != 0)
                || n.as_u64().is_some_and(|v| v != 0)
                || n.as_f64().is_some_and(|f| f.is_finite() && f != 0.0)
        }
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
    struct Holder {
        #[serde(default, deserialize_with = "lenient_i64")]
        num_i64: Option<i64>,
        #[serde(default, deserialize_with = "lenient_u64")]
        num_u64: u64,
        #[serde(default, deserialize_with = "lenient_u32")]
        num_u32: u32,
        #[serde(default, deserialize_with = "lenient_bool")]
        flag: bool,
        #[serde(default, deserialize_with = "ok_or_none")]
        opt: Option<u32>,
        #[serde(default, deserialize_with = "or_default")]
        fallback: u32,
        #[serde(default, deserialize_with = "null_default")]
        text: String,
        #[serde(default, deserialize_with = "lenient_vec")]
        items: Vec<Item>,
    }

    #[test]
    fn is_false_helper() {
        assert!(is_false(&false));
        assert!(!is_false(&true));
    }

    #[test]
    fn lenient_i64_parses_numbers_and_rejects_non_finite_floats() {
        let parse = |v: Value| {
            serde_json::from_value::<Holder>(json!({ "num_i64": v }))
                .unwrap()
                .num_i64
        };

        assert_eq!(parse(json!(42)), Some(42));
        assert_eq!(parse(json!(-7)), Some(-7));
        assert_eq!(parse(json!(12.9)), Some(12));
        assert_eq!(parse(json!("1700000000")), Some(1_700_000_000));
        assert_eq!(parse(json!(" -99 ")), Some(-99));
        assert_eq!(parse(json!("12.75")), Some(12));

        // Non-finite float strings must return None rather than 0 or i64::MAX/MIN.
        for bad in [
            "NaN",
            "nan",
            "inf",
            "-inf",
            "Infinity",
            "-Infinity",
            "abc",
            "",
        ] {
            assert_eq!(parse(json!(bad)), None, "expected None for {bad:?}");
        }
        assert_eq!(parse(json!(null)), None);
        assert_eq!(parse(json!(true)), None);
        assert_eq!(parse(json!({})), None);
        assert_eq!(parse(json!([1])), None);
    }

    #[test]
    fn lenient_u64_parses_numbers_timestamps_and_rejects_non_finite_or_negative() {
        let parse = |v: Value| {
            serde_json::from_value::<Holder>(json!({ "num_u64": v }))
                .unwrap()
                .num_u64
        };

        assert_eq!(parse(json!(42)), 42);
        assert_eq!(parse(json!(12.9)), 12);
        assert_eq!(parse(json!("1700000000")), 1_700_000_000);
        assert_eq!(parse(json!("1700000000.000100")), 1_700_000_000);
        assert_eq!(parse(json!(" 99 ")), 99);
        assert_eq!(parse(json!(u64::MAX)), u64::MAX);

        for bad in [
            json!(-1),
            json!(-5.5),
            json!("-10"),
            json!("NaN"),
            json!("inf"),
            json!("-inf"),
            json!("Infinity"),
            json!("12.abc"),
            json!("abc"),
            json!(null),
            json!({}),
        ] {
            assert_eq!(parse(bad.clone()), 0, "expected 0 for {bad:?}");
        }
    }

    #[test]
    fn lenient_u32_parses_numbers_and_strings_and_zeroes_invalid() {
        let parse = |v: Value| {
            serde_json::from_value::<Holder>(json!({ "num_u32": v }))
                .unwrap()
                .num_u32
        };

        assert_eq!(parse(json!(42)), 42);
        assert_eq!(parse(json!(12.7)), 12);
        assert_eq!(parse(json!("99")), 99);
        assert_eq!(parse(json!(u64::from(u32::MAX) + 1)), 0);
        assert_eq!(parse(json!(-1)), 0);
        assert_eq!(parse(json!("NaN")), 0);
        assert_eq!(parse(json!("inf")), 0);
        assert_eq!(parse(json!("abc")), 0);
        assert_eq!(parse(json!(null)), 0);
        assert_eq!(parse(json!({})), 0);
    }

    #[test]
    fn lenient_bool_accepts_bools_ints_and_strings() {
        let parse = |v: Value| {
            serde_json::from_value::<Holder>(json!({ "flag": v }))
                .unwrap()
                .flag
        };

        assert!(parse(json!(true)));
        assert!(!parse(json!(false)));
        assert!(parse(json!(1)));
        assert!(parse(json!(-2)));
        assert!(parse(json!(0.5)));
        assert!(!parse(json!(0)));
        assert!(!parse(json!(0.0)));
        assert!(parse(json!("1")));
        assert!(parse(json!("true")));
        assert!(!parse(json!("0")));
        assert!(!parse(json!("false")));
        assert!(!parse(json!("yes")));
        assert!(!parse(json!(null)));
        assert!(!parse(json!([])));
    }

    #[test]
    fn ok_or_none_and_or_default_tolerate_drifted_shapes() {
        let ok: Holder = serde_json::from_value(json!({ "opt": 7, "fallback": 9 })).unwrap();
        assert_eq!(ok.opt, Some(7));
        assert_eq!(ok.fallback, 9);

        for bad in [json!("not-a-u32"), json!(-5), json!(null), json!({})] {
            let h: Holder =
                serde_json::from_value(json!({ "opt": bad.clone(), "fallback": bad })).unwrap();
            assert_eq!(h.opt, None);
            assert_eq!(h.fallback, 0);
        }
        let missing: Holder = serde_json::from_value(json!({})).unwrap();
        assert_eq!(missing.opt, None);
        assert_eq!(missing.fallback, 0);
    }

    #[test]
    fn lenient_vec_skips_malformed_elements_and_handles_non_array() {
        let h: Holder = serde_json::from_value(json!({
            "items": [
                { "id": 1 },
                { "bad": true },
                "not-an-object",
                { "id": 2 }
            ]
        }))
        .unwrap();
        assert_eq!(h.items, vec![Item { id: 1 }, Item { id: 2 }]);

        let non_arr: Holder = serde_json::from_value(json!({ "items": "oops" })).unwrap();
        assert!(non_arr.items.is_empty());

        let null_arr: Holder = serde_json::from_value(json!({ "items": null })).unwrap();
        assert!(null_arr.items.is_empty());
    }

    #[test]
    fn null_default_defaults_null_and_missing_but_errors_on_wrong_type() {
        let from_str: Holder = serde_json::from_value(json!({ "text": "hi" })).unwrap();
        assert_eq!(from_str.text, "hi");

        let from_null: Holder = serde_json::from_value(json!({ "text": null })).unwrap();
        assert_eq!(from_null.text, "");

        let from_missing: Holder = serde_json::from_value(json!({})).unwrap();
        assert_eq!(from_missing.text, "");

        assert!(serde_json::from_value::<Holder>(json!({ "text": 123 })).is_err());
    }
}
