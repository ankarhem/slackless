//! Lenient deserializers for response fields whose shape Slack varies.
//!
//! Shared serde deserializers live in [`slackless_types::de`]; this module re-exports them and
//! provides `serde_json::Value` helpers used by wire parsers.

use serde_json::Value;

pub(crate) use slackless_types::de::{
    lenient_bool, lenient_u32, lenient_u64, lenient_vec, null_default, or_default,
};

/// Parses a `serde_json::Value` as a non-negative `u64` (integer, finite non-negative float,
/// or numeric / timestamp string).
pub(crate) fn value_as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite() && *f >= 0.0 && *f < (u64::MAX as f64))
                .map(|f| f as u64)
        }),
        Value::String(s) => s.trim().split('.').next().and_then(|w| w.parse().ok()),
        _ => None,
    }
}

/// Parses a `serde_json::Value` as a lenient bool (`bool`, non-zero integer, or `"1"`/`"true"` /
/// `"0"`/`"false"`). Returns `None` for unrecognized or `null` values.
pub(crate) fn value_as_lenient_bool(val: &Value) -> Option<bool> {
    match val {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_i64().map(|i| i != 0),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
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
    fn value_as_u64_and_lenient_bool_handle_edge_cases() {
        assert_eq!(value_as_u64(&json!(42)), Some(42));
        assert_eq!(value_as_u64(&json!(42.9)), Some(42));
        assert_eq!(value_as_u64(&json!(-1.0)), None);
        assert_eq!(value_as_u64(&json!(1e30)), None);
        assert_eq!(
            value_as_u64(&json!(" 1700000000.123 ")),
            Some(1_700_000_000)
        );
        assert_eq!(value_as_u64(&json!(null)), None);

        assert_eq!(value_as_lenient_bool(&json!(true)), Some(true));
        assert_eq!(value_as_lenient_bool(&json!("TRUE")), Some(true));
        assert_eq!(value_as_lenient_bool(&json!("0")), Some(false));
        assert_eq!(value_as_lenient_bool(&json!("yes")), None);
        assert_eq!(value_as_lenient_bool(&json!(null)), None);
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
