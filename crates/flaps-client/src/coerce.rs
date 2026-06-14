//! Type coercion from `serde_json::Value` to concrete OpenFeature value types.
//!
//! Every function returns `None` on type mismatch so callers can emit
//! [`EvaluationErrorCode::TypeMismatch`] without panicking.

use open_feature::StructValue;

/// `i64::MIN` expressed as `f64` (exactly representable).
const I64_MIN_F64: f64 = -9_223_372_036_854_775_808.0_f64;

/// Largest `f64` value that can be cast to `i64` without truncation.
///
/// `i64::MAX` (2^63 - 1) rounds up to 2^63 when converted to `f64`, which
/// overflows on cast back. The previous exactly-representable `f64` is
/// 2^63 - 1024.
const I64_MAX_SAFE_F64: f64 = 9_223_372_036_854_774_784.0_f64;

/// Coerces a JSON value to `bool`. Returns `None` when the value is not a boolean.
#[must_use]
pub(crate) fn to_bool(value: &serde_json::Value) -> Option<bool> {
    value.as_bool()
}

/// Coerces a JSON value to `i64`.
///
/// Accepts JSON numbers that are already stored as integers, and JSON numbers
/// stored as `f64` whose value is exactly representable as `i64` (e.g. `1.0`).
/// Returns `None` on type mismatch or when the floating-point value has a
/// fractional part.
#[must_use]
pub(crate) fn to_int(value: &serde_json::Value) -> Option<i64> {
    if let Some(i) = value.as_i64() {
        return Some(i);
    }
    // flagd stores all numeric variants as f64 internally; a value such as
    // `1.0` round-trips through serde_json as a float-typed Number. Accept it
    // when the float is an exact integer.
    value.as_f64().and_then(|f| {
        if f.fract() == 0.0 && (I64_MIN_F64..=I64_MAX_SAFE_F64).contains(&f) {
            #[allow(clippy::cast_possible_truncation)]
            Some(f as i64)
        } else {
            None
        }
    })
}

/// Coerces a JSON value to `f64`. Returns `None` when the value is not a number.
#[must_use]
pub(crate) fn to_float(value: &serde_json::Value) -> Option<f64> {
    value.as_f64()
}

/// Coerces a JSON value to `String`. Returns `None` when the value is not a string.
#[must_use]
pub(crate) fn to_string(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(ToOwned::to_owned)
}

/// Coerces a JSON value to [`StructValue`].
///
/// Only JSON objects are accepted; any other shape returns `None`.
#[must_use]
pub(crate) fn to_struct(value: &serde_json::Value) -> Option<StructValue> {
    let obj = value.as_object()?;
    let mut sv = StructValue::default();
    for (key, val) in obj {
        if let Some(of_val) = json_to_of_value(val) {
            sv.add_field(key.clone(), of_val);
        }
    }
    Some(sv)
}

/// Recursively converts a `serde_json::Value` to an OpenFeature [`Value`].
///
/// Returns `None` for JSON values that have no OpenFeature counterpart (e.g.
/// top-level `Null`).
fn json_to_of_value(value: &serde_json::Value) -> Option<open_feature::Value> {
    match value {
        serde_json::Value::Bool(b) => Some(open_feature::Value::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(open_feature::Value::Int(i))
            } else {
                n.as_f64().map(open_feature::Value::Float)
            }
        }
        serde_json::Value::String(s) => Some(open_feature::Value::String(s.clone())),
        serde_json::Value::Array(arr) => {
            let converted: Vec<open_feature::Value> =
                arr.iter().filter_map(json_to_of_value).collect();
            Some(open_feature::Value::Array(converted))
        }
        serde_json::Value::Object(_) => to_struct(value).map(open_feature::Value::Struct),
        serde_json::Value::Null => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bool_true() {
        assert_eq!(to_bool(&json!(true)), Some(true));
    }

    #[test]
    fn bool_false() {
        assert_eq!(to_bool(&json!(false)), Some(false));
    }

    #[test]
    fn bool_mismatch_string() {
        assert_eq!(to_bool(&json!("yes")), None);
    }

    #[test]
    fn bool_mismatch_number() {
        assert_eq!(to_bool(&json!(1)), None);
    }

    #[test]
    fn int_ok() {
        assert_eq!(to_int(&json!(42)), Some(42));
    }

    #[test]
    fn int_from_float_whole() {
        // flagd stores numeric variants as f64; 1.0 must coerce to i64 1
        let v =
            serde_json::Value::Number(serde_json::Number::from_f64(1.0).expect("1.0 is finite"));
        assert_eq!(to_int(&v), Some(1));
    }

    #[test]
    fn int_from_float_fractional_is_none() {
        let v =
            serde_json::Value::Number(serde_json::Number::from_f64(1.5).expect("1.5 is finite"));
        assert_eq!(to_int(&v), None);
    }

    #[test]
    fn int_mismatch_string() {
        assert_eq!(to_int(&json!("42")), None);
    }

    #[test]
    fn int_mismatch_bool() {
        assert_eq!(to_int(&json!(true)), None);
    }

    #[test]
    fn float_ok() {
        assert!((to_float(&json!(1.5)).unwrap() - 1.5_f64).abs() < f64::EPSILON);
    }

    #[test]
    fn float_mismatch_string() {
        assert_eq!(to_float(&json!("3.14")), None);
    }

    #[test]
    fn string_ok() {
        assert_eq!(to_string(&json!("hello")), Some("hello".to_owned()));
    }

    #[test]
    fn string_mismatch_bool() {
        assert_eq!(to_string(&json!(true)), None);
    }

    #[test]
    fn struct_ok() {
        let val = json!({"key": "value"});
        let sv = to_struct(&val).expect("should convert object");
        assert!(sv.fields.contains_key("key"));
    }

    #[test]
    fn struct_mismatch_primitive() {
        assert!(to_struct(&json!(42)).is_none());
        assert!(to_struct(&json!("hello")).is_none());
        assert!(to_struct(&json!(true)).is_none());
    }
}
