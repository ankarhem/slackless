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
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)),
        Some(Value::String(s)) => s.parse::<i64>().ok().or_else(|| {
            s.parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .map(|f| f as i64)
        }),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Default, PartialEq, Deserialize)]
    struct Holder {
        #[serde(default, deserialize_with = "lenient_i64")]
        num: Option<i64>,
        #[serde(default, deserialize_with = "ok_or_none")]
        opt: Option<u32>,
        #[serde(default, deserialize_with = "null_default")]
        text: String,
    }

    #[test]
    fn lenient_i64_parses_numbers_and_rejects_non_finite_floats() {
        let parse = |v: Value| {
            serde_json::from_value::<Holder>(json!({ "num": v }))
                .unwrap()
                .num
        };

        assert_eq!(parse(json!(42)), Some(42));
        assert_eq!(parse(json!(-7)), Some(-7));
        assert_eq!(parse(json!(12.9)), Some(12));
        assert_eq!(parse(json!("1700000000")), Some(1_700_000_000));
        assert_eq!(parse(json!("-99")), Some(-99));
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
    fn ok_or_none_tolerates_drifted_shapes() {
        let ok: Holder = serde_json::from_value(json!({ "opt": 7 })).unwrap();
        assert_eq!(ok.opt, Some(7));

        for bad in [json!("not-a-u32"), json!(-5), json!(null), json!({})] {
            let h: Holder = serde_json::from_value(json!({ "opt": bad })).unwrap();
            assert_eq!(h.opt, None);
        }
        let missing: Holder = serde_json::from_value(json!({})).unwrap();
        assert_eq!(missing.opt, None);
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
