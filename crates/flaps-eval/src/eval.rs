//! Evaluation of parsed flag sets against an evaluation context.
//!
//! Follows the flagd evaluation semantics. Disabled flags short-circuit with
//! reason [`Reason::Disabled`] and carry no value or variant. Flags without
//! targeting resolve the default variant with reason [`Reason::Static`].
//! Targeting rules resolve a variant with reason [`Reason::TargetingMatch`],
//! or fall back to the default variant with reason [`Reason::Default`] when
//! the rule returns `null`.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::model::{FlagSet, Metadata, State, Variants};

/// The context a targeting rule evaluates against.
///
/// The targeting key is exposed to rules as the `targetingKey` attribute.
/// The timestamp is exposed as `$flagd.timestamp` and is supplied by the
/// caller so evaluation stays deterministic and testable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvaluationContext {
    /// Identifier of the evaluation subject, exposed as `targetingKey`.
    pub targeting_key: Option<String>,
    /// Arbitrary context attributes addressed by the `var` operator.
    pub attributes: BTreeMap<String, serde_json::Value>,
    /// Unix timestamp in seconds, exposed as `$flagd.timestamp`.
    pub timestamp: u64,
}

/// Why an evaluation resolved the way it did.
///
/// Mirrors the OpenFeature resolution reasons produced by flagd providers.
/// The OpenFeature `ERROR` reason has no variant here: failed evaluations
/// are represented by the [`EvaluationError`] returned by
/// [`FlagSet::evaluate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The flag has no targeting rule; the default variant was served.
    Static,
    /// The targeting rule selected a variant.
    TargetingMatch,
    /// The targeting rule returned `null`; the default variant was served.
    Default,
    /// The flag is disabled; the caller serves its own code default.
    Disabled,
}

/// The outcome of a successful evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// The resolved variant value.
    ///
    /// `None` when the flag is disabled, and when no variant was resolved
    /// from targeting while the flag defines no default variant. In both
    /// cases the caller serves its own code default.
    pub value: Option<serde_json::Value>,
    /// The key of the resolved variant, when one was resolved.
    pub variant: Option<String>,
    /// Why this resolution was produced.
    pub reason: Reason,
    /// Flag set metadata merged with flag metadata, flag entries winning.
    pub metadata: Metadata,
}

/// An error produced while evaluating a flag.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EvaluationError {
    /// The requested flag key is not present in the flag set.
    #[error("flag `{flag_key}` not found")]
    FlagNotFound {
        /// The missing flag key.
        flag_key: String,
    },

    /// The targeting rule resolved to a value that selects no variant.
    ///
    /// Targeting rules must resolve to a variant key, a boolean mapping to
    /// the `"true"` or `"false"` keys, or `null` to exit to the default
    /// variant. Any other value is an evaluation error.
    #[error("targeting of flag `{flag_key}` resolved to an invalid variant: {resolved}")]
    InvalidVariant {
        /// Key of the offending flag.
        flag_key: String,
        /// The value the targeting rule resolved to.
        resolved: serde_json::Value,
    },

    /// The rule uses a flagd custom operation not implemented yet.
    ///
    /// Temporary variant covering `fractional`, `sem_ver`, `starts_with`
    /// and `ends_with` until the custom operations land.
    #[error("custom operation `{operator}` is not implemented yet")]
    UnsupportedOperation {
        /// Name of the unimplemented operator.
        operator: &'static str,
    },
}

impl FlagSet {
    /// Evaluates a flag of this set against an evaluation context.
    ///
    /// Evaluation never panics: structurally valid rules always produce a
    /// value, and adversarial context values degrade to falsy or nullish
    /// results per the JsonLogic semantics.
    ///
    /// # Errors
    ///
    /// Returns [`EvaluationError::FlagNotFound`] for an unknown flag key,
    /// [`EvaluationError::InvalidVariant`] when targeting resolves to a
    /// value that selects no variant, and
    /// [`EvaluationError::UnsupportedOperation`] when the rule reaches a
    /// custom operation that is not implemented yet.
    pub fn evaluate(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> Result<Resolution, EvaluationError> {
        let flag = self
            .flags
            .get(flag_key)
            .ok_or_else(|| EvaluationError::FlagNotFound {
                flag_key: flag_key.to_owned(),
            })?;
        match flag.state {
            State::Disabled => todo!(),
            State::Enabled => {}
        }
        let Some(targeting) = &flag.targeting else {
            todo!()
        };
        let scope = evaluation_scope(flag_key, context);
        let outcome = crate::logic::apply(targeting, &scope)?;
        let (variant, reason) = match outcome {
            Value::Bool(boolean) => (boolean.to_string(), Reason::TargetingMatch),
            Value::Null => match &flag.default_variant {
                Some(default) => (default.clone(), Reason::Default),
                None => todo!(),
            },
            _ => todo!(),
        };
        let Some(value) = variant_value(&flag.variants, &variant) else {
            todo!()
        };
        Ok(Resolution {
            value: Some(value),
            variant: Some(variant),
            reason,
            metadata: Metadata::new(),
        })
    }
}

/// Builds the data scope rules evaluate against: the context attributes,
/// the targeting key under `targetingKey`, and the reserved `$flagd`
/// object carrying the flag key and the timestamp.
fn evaluation_scope(flag_key: &str, context: &EvaluationContext) -> Value {
    let mut scope = serde_json::Map::new();
    for (key, value) in &context.attributes {
        scope.insert(key.clone(), value.clone());
    }
    if let Some(targeting_key) = &context.targeting_key {
        scope.insert("targetingKey".to_owned(), targeting_key.clone().into());
    }
    scope.insert(
        "$flagd".to_owned(),
        json!({ "flagKey": flag_key, "timestamp": context.timestamp }),
    );
    Value::Object(scope)
}

/// Looks a variant key up and converts its value to JSON.
fn variant_value(variants: &Variants, name: &str) -> Option<Value> {
    match variants {
        Variants::Boolean(map) => map.get(name).map(|value| Value::Bool(*value)),
        Variants::String(map) => map.get(name).map(|value| Value::String(value.clone())),
        Variants::Number(map) => map
            .get(name)
            .map(|value| serde_json::Number::from_f64(*value).map_or(Value::Null, Value::Number)),
        Variants::Object(map) => map.get(name).map(|value| Value::Object(value.clone())),
    }
}
