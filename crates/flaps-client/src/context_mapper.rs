//! Maps an OpenFeature [`EvaluationContext`] to a flaps-eval context.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use open_feature::{
    EvaluationContext, EvaluationContextFieldValue, EvaluationError, EvaluationErrorCode,
    EvaluationResult, StructValue, Value as OpenFeatureValue,
};

/// Converts an OpenFeature evaluation context to a flaps-eval evaluation context.
///
/// The targeting key is forwarded as-is. Custom fields are mapped to
/// `serde_json::Value` recursively, so that the resulting attributes match
/// exactly what the OFREP HTTP endpoint would receive for the same logical
/// context: primitives convert to their JSON counterpart, and
/// [`EvaluationContextFieldValue::Struct`] payloads that wrap a supported
/// structured type ([`StructValue`], [`OpenFeatureValue`], or a raw
/// [`serde_json::Value`]) convert to a nested JSON object or array. This
/// preserves the remote/local parity guarantee for rules that target nested
/// fields (e.g. `user.plan`).
///
/// The `timestamp` is set to the current UNIX second so evaluation rules that
/// depend on time see a consistent value within a single evaluation call.
///
/// # Errors
///
/// Returns [`EvaluationErrorCode::InvalidContext`] when a custom field is a
/// [`EvaluationContextFieldValue::Struct`] wrapping a type other than
/// [`StructValue`], [`OpenFeatureValue`], or [`serde_json::Value`]. Such a
/// payload is opaque (type-erased behind `Arc<dyn Any>`) and cannot be
/// introspected into JSON; the error is surfaced to the caller instead of
/// silently dropping the field.
pub(crate) fn map_context(
    of_ctx: &EvaluationContext,
) -> EvaluationResult<flaps_eval::EvaluationContext> {
    let targeting_key = of_ctx.targeting_key.clone();
    let mut attributes: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    for (key, value) in &of_ctx.custom_fields {
        if let Some(json_val) = field_to_json(key, value)? {
            attributes.insert(key.clone(), json_val);
        }
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    Ok(flaps_eval::EvaluationContext {
        targeting_key,
        attributes,
        timestamp,
    })
}

/// Converts a single [`EvaluationContextFieldValue`] to `serde_json::Value`.
///
/// Returns `Ok(None)` for a non-finite float, which has no JSON
/// representation and is omitted from the attribute map, matching the
/// pre-existing lenient behaviour for that primitive. Returns `Err` when a
/// `Struct` payload cannot be recognised; see [`map_context`] for the
/// rationale.
fn field_to_json(
    key: &str,
    value: &EvaluationContextFieldValue,
) -> EvaluationResult<Option<serde_json::Value>> {
    let json = match value {
        EvaluationContextFieldValue::Bool(b) => Some(serde_json::Value::Bool(*b)),
        EvaluationContextFieldValue::Int(i) => Some(serde_json::Value::Number((*i).into())),
        EvaluationContextFieldValue::Float(f) => {
            serde_json::Number::from_f64(*f).map(serde_json::Value::Number)
        }
        EvaluationContextFieldValue::String(s) => Some(serde_json::Value::String(s.clone())),
        EvaluationContextFieldValue::DateTime(dt) => {
            // Wire representation: integer Unix timestamp in seconds. This is
            // the CANONICAL wire representation for `DateTime` context
            // fields: any future OFREP client-side serializer MUST encode
            // `DateTime` fields the same way for remote/local evaluation
            // results to stay in parity with the value produced here.
            Some(serde_json::Value::Number(dt.unix_timestamp().into()))
        }
        EvaluationContextFieldValue::Struct(opaque) => Some(struct_field_to_json(key, opaque)?),
    };
    Ok(json)
}

/// Converts an opaque `Struct` payload to JSON.
///
/// Recognises the three structured value types this module supports:
/// [`StructValue`] (a JSON object), [`OpenFeatureValue`] (which may itself be
/// an array, a nested struct, or a primitive), and a raw [`serde_json::Value`]
/// (already JSON, so it is cloned through unchanged). The last one covers the
/// most common way a Rust caller supplies structured context data directly,
/// and is exactly what the OFREP HTTP path carries for the same logical
/// payload, so recognising it here closes a remote/local parity gap. Any
/// other type cannot be inspected through `Any` and is reported as
/// [`EvaluationErrorCode::InvalidContext`] rather than silently dropped.
fn struct_field_to_json(
    key: &str,
    opaque: &Arc<dyn Any + Send + Sync>,
) -> EvaluationResult<serde_json::Value> {
    if let Some(struct_value) = opaque.downcast_ref::<StructValue>() {
        return Ok(struct_value_to_json(struct_value));
    }
    if let Some(value) = opaque.downcast_ref::<OpenFeatureValue>() {
        return Ok(open_feature_value_to_json(value));
    }
    if let Some(json_value) = opaque.downcast_ref::<serde_json::Value>() {
        return Ok(json_value.clone());
    }
    Err(EvaluationError {
        code: EvaluationErrorCode::InvalidContext,
        message: Some(format!(
            "custom field `{key}` holds an opaque struct value that is neither \
             `open_feature::StructValue`, `open_feature::Value`, nor \
             `serde_json::Value`; wrap structured data in one of those types so \
             it can be converted to JSON"
        )),
    })
}

/// Recursively converts an [`OpenFeatureValue`] to `serde_json::Value`.
///
/// Non-finite floats convert to `null`: unlike the top-level primitive case
/// in [`field_to_json`], a value nested inside an object or array cannot be
/// omitted without invalidating the surrounding JSON structure.
fn open_feature_value_to_json(value: &OpenFeatureValue) -> serde_json::Value {
    match value {
        OpenFeatureValue::Bool(b) => serde_json::Value::Bool(*b),
        OpenFeatureValue::Int(i) => serde_json::Value::Number((*i).into()),
        OpenFeatureValue::Float(f) => serde_json::Number::from_f64(*f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        OpenFeatureValue::String(s) => serde_json::Value::String(s.clone()),
        OpenFeatureValue::Array(items) => {
            serde_json::Value::Array(items.iter().map(open_feature_value_to_json).collect())
        }
        OpenFeatureValue::Struct(struct_value) => struct_value_to_json(struct_value),
    }
}

/// Converts a [`StructValue`] to a `serde_json::Value::Object`.
fn struct_value_to_json(struct_value: &StructValue) -> serde_json::Value {
    serde_json::Value::Object(
        struct_value
            .fields
            .iter()
            .map(|(field_key, field_value)| {
                (field_key.clone(), open_feature_value_to_json(field_value))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use open_feature::{EvaluationContext, EvaluationErrorCode, StructValue, Value as OfValue};
    use time::OffsetDateTime;

    use super::*;

    #[test]
    fn maps_targeting_key() {
        let of_ctx = EvaluationContext::default().with_targeting_key("user-42");
        let eval_ctx = map_context(&of_ctx).expect("no struct fields to fail on");
        assert_eq!(eval_ctx.targeting_key, Some("user-42".to_owned()));
    }

    #[test]
    fn maps_bool_field() {
        let of_ctx = EvaluationContext::default().with_custom_field("premium", true);
        let eval_ctx = map_context(&of_ctx).expect("no struct fields to fail on");
        assert_eq!(
            eval_ctx.attributes.get("premium"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn maps_int_field() {
        let of_ctx = EvaluationContext::default().with_custom_field("age", 30_i64);
        let eval_ctx = map_context(&of_ctx).expect("no struct fields to fail on");
        assert_eq!(
            eval_ctx.attributes.get("age"),
            Some(&serde_json::json!(30_i64))
        );
    }

    #[test]
    fn maps_string_field() {
        let of_ctx = EvaluationContext::default().with_custom_field("country", "FR");
        let eval_ctx = map_context(&of_ctx).expect("no struct fields to fail on");
        assert_eq!(
            eval_ctx.attributes.get("country"),
            Some(&serde_json::Value::String("FR".to_owned()))
        );
    }

    #[test]
    fn empty_context_produces_empty_attributes() {
        let of_ctx = EvaluationContext::default();
        let eval_ctx = map_context(&of_ctx).expect("empty context never fails");
        assert!(eval_ctx.targeting_key.is_none());
        assert!(eval_ctx.attributes.is_empty());
    }

    // -------------------------------------------------------------------
    // Structured fields (issue #106): `StructValue` objects and `Value`
    // arrays must convert recursively instead of being dropped.
    // -------------------------------------------------------------------

    #[test]
    fn maps_struct_field_to_nested_json_object() {
        let of_ctx = EvaluationContext::default().with_custom_field(
            "user",
            EvaluationContextFieldValue::new_struct(
                StructValue::default().with_field("plan", "pro"),
            ),
        );
        let eval_ctx = map_context(&of_ctx).expect("StructValue is a supported structured type");
        assert_eq!(
            eval_ctx.attributes.get("user"),
            Some(&serde_json::json!({ "plan": "pro" }))
        );
    }

    #[test]
    fn maps_struct_field_recursively_for_nested_structs() {
        let inner = StructValue::default().with_field("plan", "pro");
        let outer = StructValue::default().with_field("user", inner);
        let of_ctx = EvaluationContext::default()
            .with_custom_field("account", EvaluationContextFieldValue::new_struct(outer));
        let eval_ctx = map_context(&of_ctx).expect("nested StructValue is supported");
        assert_eq!(
            eval_ctx.attributes.get("account"),
            Some(&serde_json::json!({ "user": { "plan": "pro" } }))
        );
    }

    #[test]
    fn maps_value_array_field_to_json_array() {
        let of_ctx = EvaluationContext::default().with_custom_field(
            "tags",
            EvaluationContextFieldValue::new_struct(OfValue::Array(vec![
                OfValue::String("vip".to_owned()),
                OfValue::String("beta".to_owned()),
            ])),
        );
        let eval_ctx = map_context(&of_ctx).expect("Value::Array is a supported structured type");
        assert_eq!(
            eval_ctx.attributes.get("tags"),
            Some(&serde_json::json!(["vip", "beta"]))
        );
    }

    #[test]
    fn maps_struct_field_with_nested_array_member() {
        let of_ctx = EvaluationContext::default().with_custom_field(
            "user",
            EvaluationContextFieldValue::new_struct(
                StructValue::default().with_field("tags", vec!["vip", "beta"]),
            ),
        );
        let eval_ctx = map_context(&of_ctx).expect("nested array member is supported");
        assert_eq!(
            eval_ctx.attributes.get("user"),
            Some(&serde_json::json!({ "tags": ["vip", "beta"] }))
        );
    }

    #[test]
    fn maps_raw_serde_json_value_struct_field_to_nested_json_object() {
        let of_ctx = EvaluationContext::default().with_custom_field(
            "user",
            EvaluationContextFieldValue::new_struct(serde_json::json!({ "plan": "pro" })),
        );
        let eval_ctx =
            map_context(&of_ctx).expect("a raw serde_json::Value struct field is supported");
        assert_eq!(
            eval_ctx.attributes.get("user"),
            Some(&serde_json::json!({ "plan": "pro" }))
        );
    }

    #[test]
    fn opaque_struct_type_is_reported_as_invalid_context_not_dropped() {
        let of_ctx = EvaluationContext::default()
            .with_custom_field("opaque", EvaluationContextFieldValue::new_struct(42_u32));
        let err = map_context(&of_ctx)
            .expect_err("an opaque, non-introspectable struct must surface an error");
        assert_eq!(err.code, EvaluationErrorCode::InvalidContext);
        assert!(err.message.is_some_and(|m| m.contains("opaque")));
    }

    #[test]
    fn maps_datetime_field_to_unix_timestamp_seconds() {
        let joined = OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .expect("valid unix timestamp seconds");
        let of_ctx = EvaluationContext::default().with_custom_field("joined", joined);
        let eval_ctx = map_context(&of_ctx).expect("DateTime always maps");
        assert_eq!(
            eval_ctx.attributes.get("joined"),
            Some(&serde_json::json!(1_700_000_000_i64))
        );
    }

    // -------------------------------------------------------------------
    // Parity oracle (issue #106): the JSON `flaps-client` produces for a
    // structured/DateTime custom field must evaluate identically to the
    // JSON the OFREP endpoint builds for the same logical payload. OFREP
    // deserializes the request body straight into
    // `BTreeMap<String, serde_json::Value>` and hands it to
    // `flaps_eval::EvaluationContext.attributes` unmodified (see
    // `flaps-server/src/routes/ofrep.rs::build_context`), so constructing
    // that map directly from the equivalent JSON *is* the OFREP path,
    // without needing a live server.
    // -------------------------------------------------------------------

    /// A flagd document whose targeting rule reads a nested object field.
    const NESTED_OBJECT_DOCUMENT: &str = r#"{
        "flags": {
            "pro-only": {
                "state": "ENABLED",
                "variants": { "true": true, "false": false },
                "defaultVariant": "false",
                "targeting": { "==": [{ "var": "user.plan" }, "pro"] }
            }
        }
    }"#;

    /// A flagd document whose targeting rule reads a nested array field.
    const NESTED_ARRAY_DOCUMENT: &str = r#"{
        "flags": {
            "vip-only": {
                "state": "ENABLED",
                "variants": { "true": true, "false": false },
                "defaultVariant": "false",
                "targeting": { "in": ["vip", { "var": "tags" }] }
            }
        }
    }"#;

    /// A flagd document whose targeting rule reads a `DateTime` field.
    const AFTER_LAUNCH_DOCUMENT: &str = r#"{
        "flags": {
            "after-launch": {
                "state": "ENABLED",
                "variants": { "true": true, "false": false },
                "defaultVariant": "false",
                "targeting": { ">": [{ "var": "joined" }, 1699999999] }
            }
        }
    }"#;

    #[test]
    fn nested_object_field_evaluates_identically_to_the_ofrep_json_shape() {
        let of_ctx = EvaluationContext::default().with_custom_field(
            "user",
            EvaluationContextFieldValue::new_struct(
                StructValue::default().with_field("plan", "pro"),
            ),
        );
        let local_ctx = map_context(&of_ctx).expect("StructValue maps to json");
        let flag_set = flaps_eval::FlagSet::from_json(NESTED_OBJECT_DOCUMENT)
            .expect("valid flag set document");
        let local_resolution = flag_set
            .evaluate("pro-only", &local_ctx)
            .expect("evaluation succeeds");

        let mut oracle_attributes = BTreeMap::new();
        oracle_attributes.insert("user".to_owned(), serde_json::json!({ "plan": "pro" }));
        let oracle_ctx = flaps_eval::EvaluationContext {
            targeting_key: None,
            attributes: oracle_attributes,
            timestamp: local_ctx.timestamp,
        };
        let oracle_resolution = flag_set
            .evaluate("pro-only", &oracle_ctx)
            .expect("evaluation succeeds");

        assert_eq!(local_resolution, oracle_resolution);
        assert_eq!(local_resolution.value, Some(serde_json::Value::Bool(true)));
    }

    #[test]
    fn raw_serde_json_value_struct_field_evaluates_identically_to_the_ofrep_json_shape() {
        let of_ctx = EvaluationContext::default().with_custom_field(
            "user",
            EvaluationContextFieldValue::new_struct(serde_json::json!({ "plan": "pro" })),
        );
        let local_ctx = map_context(&of_ctx).expect("serde_json::Value maps to json");
        let flag_set = flaps_eval::FlagSet::from_json(NESTED_OBJECT_DOCUMENT)
            .expect("valid flag set document");
        let local_resolution = flag_set
            .evaluate("pro-only", &local_ctx)
            .expect("evaluation succeeds");

        let mut oracle_attributes = BTreeMap::new();
        oracle_attributes.insert("user".to_owned(), serde_json::json!({ "plan": "pro" }));
        let oracle_ctx = flaps_eval::EvaluationContext {
            targeting_key: None,
            attributes: oracle_attributes,
            timestamp: local_ctx.timestamp,
        };
        let oracle_resolution = flag_set
            .evaluate("pro-only", &oracle_ctx)
            .expect("evaluation succeeds");

        assert_eq!(local_resolution, oracle_resolution);
        assert_eq!(local_resolution.value, Some(serde_json::Value::Bool(true)));
    }

    #[test]
    fn nested_array_field_evaluates_identically_to_the_ofrep_json_shape() {
        let of_ctx = EvaluationContext::default().with_custom_field(
            "tags",
            EvaluationContextFieldValue::new_struct(OfValue::Array(vec![
                OfValue::String("vip".to_owned()),
                OfValue::String("beta".to_owned()),
            ])),
        );
        let local_ctx = map_context(&of_ctx).expect("Value::Array maps to json");
        let flag_set =
            flaps_eval::FlagSet::from_json(NESTED_ARRAY_DOCUMENT).expect("valid flag set document");
        let local_resolution = flag_set
            .evaluate("vip-only", &local_ctx)
            .expect("evaluation succeeds");

        let mut oracle_attributes = BTreeMap::new();
        oracle_attributes.insert("tags".to_owned(), serde_json::json!(["vip", "beta"]));
        let oracle_ctx = flaps_eval::EvaluationContext {
            targeting_key: None,
            attributes: oracle_attributes,
            timestamp: local_ctx.timestamp,
        };
        let oracle_resolution = flag_set
            .evaluate("vip-only", &oracle_ctx)
            .expect("evaluation succeeds");

        assert_eq!(local_resolution, oracle_resolution);
        assert_eq!(local_resolution.value, Some(serde_json::Value::Bool(true)));
    }

    #[test]
    fn datetime_field_evaluates_identically_to_the_ofrep_json_shape() {
        let joined = OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .expect("valid unix timestamp seconds");
        let of_ctx = EvaluationContext::default().with_custom_field("joined", joined);
        let local_ctx = map_context(&of_ctx).expect("DateTime always maps");
        let flag_set =
            flaps_eval::FlagSet::from_json(AFTER_LAUNCH_DOCUMENT).expect("valid flag set document");
        let local_resolution = flag_set
            .evaluate("after-launch", &local_ctx)
            .expect("evaluation succeeds");

        let mut oracle_attributes = BTreeMap::new();
        oracle_attributes.insert("joined".to_owned(), serde_json::json!(1_700_000_000_i64));
        let oracle_ctx = flaps_eval::EvaluationContext {
            targeting_key: None,
            attributes: oracle_attributes,
            timestamp: local_ctx.timestamp,
        };
        let oracle_resolution = flag_set
            .evaluate("after-launch", &oracle_ctx)
            .expect("evaluation succeeds");

        assert_eq!(local_resolution, oracle_resolution);
        assert_eq!(local_resolution.value, Some(serde_json::Value::Bool(true)));
    }
}
