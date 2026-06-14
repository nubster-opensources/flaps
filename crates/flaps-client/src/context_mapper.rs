//! Maps an OpenFeature [`EvaluationContext`] to a flaps-eval context.

use std::collections::BTreeMap;

use open_feature::{EvaluationContext, EvaluationContextFieldValue};

/// Converts an OpenFeature evaluation context to a flaps-eval evaluation context.
///
/// The targeting key is forwarded as-is. Custom fields are mapped to
/// `serde_json::Value` on a best-effort basis: struct fields are dropped
/// because they cannot be serialised to a JSON primitive.
///
/// The `timestamp` is set to the current UNIX second so evaluation rules that
/// depend on time see a consistent value within a single evaluation call.
#[must_use]
pub(crate) fn map_context(of_ctx: &EvaluationContext) -> flaps_eval::EvaluationContext {
    let targeting_key = of_ctx.targeting_key.clone();
    let mut attributes: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    for (key, value) in &of_ctx.custom_fields {
        if let Some(json_val) = field_to_json(value) {
            attributes.insert(key.clone(), json_val);
        }
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    flaps_eval::EvaluationContext {
        targeting_key,
        attributes,
        timestamp,
    }
}

/// Converts a single [`EvaluationContextFieldValue`] to `serde_json::Value`.
///
/// Returns `None` for struct values, which cannot be represented as JSON.
fn field_to_json(value: &EvaluationContextFieldValue) -> Option<serde_json::Value> {
    match value {
        EvaluationContextFieldValue::Bool(b) => Some(serde_json::Value::Bool(*b)),
        EvaluationContextFieldValue::Int(i) => Some(serde_json::Value::Number((*i).into())),
        EvaluationContextFieldValue::Float(f) => {
            serde_json::Number::from_f64(*f).map(serde_json::Value::Number)
        }
        EvaluationContextFieldValue::String(s) => Some(serde_json::Value::String(s.clone())),
        EvaluationContextFieldValue::DateTime(dt) => {
            Some(serde_json::Value::Number(dt.unix_timestamp().into()))
        }
        EvaluationContextFieldValue::Struct(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_feature::EvaluationContext;

    #[test]
    fn maps_targeting_key() {
        let of_ctx = EvaluationContext::default().with_targeting_key("user-42");
        let eval_ctx = map_context(&of_ctx);
        assert_eq!(eval_ctx.targeting_key, Some("user-42".to_owned()));
    }

    #[test]
    fn maps_bool_field() {
        let of_ctx = EvaluationContext::default().with_custom_field("premium", true);
        let eval_ctx = map_context(&of_ctx);
        assert_eq!(
            eval_ctx.attributes.get("premium"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn maps_int_field() {
        let of_ctx = EvaluationContext::default().with_custom_field("age", 30_i64);
        let eval_ctx = map_context(&of_ctx);
        assert_eq!(
            eval_ctx.attributes.get("age"),
            Some(&serde_json::json!(30_i64))
        );
    }

    #[test]
    fn maps_string_field() {
        let of_ctx = EvaluationContext::default().with_custom_field("country", "FR");
        let eval_ctx = map_context(&of_ctx);
        assert_eq!(
            eval_ctx.attributes.get("country"),
            Some(&serde_json::Value::String("FR".to_owned()))
        );
    }

    #[test]
    fn drops_struct_field_silently() {
        use std::sync::Arc;
        let of_ctx = EvaluationContext::default().with_custom_field(
            "opaque",
            EvaluationContextFieldValue::Struct(Arc::new(42_u32)),
        );
        let eval_ctx = map_context(&of_ctx);
        assert!(!eval_ctx.attributes.contains_key("opaque"));
    }

    #[test]
    fn empty_context_produces_empty_attributes() {
        let of_ctx = EvaluationContext::default();
        let eval_ctx = map_context(&of_ctx);
        assert!(eval_ctx.targeting_key.is_none());
        assert!(eval_ctx.attributes.is_empty());
    }
}
