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
    use serde_json::json;

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
}
