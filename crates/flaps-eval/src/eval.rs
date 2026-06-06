//! Evaluation of parsed flag sets against an evaluation context.
//!
//! Follows the flagd evaluation semantics. Disabled flags short-circuit with
//! reason [`Reason::Disabled`] and carry no value or variant. Flags without
//! targeting resolve the default variant with reason [`Reason::Static`].
//! Targeting rules resolve a variant with reason [`Reason::TargetingMatch`],
//! or fall back to the default variant with reason [`Reason::Default`] when
//! the rule returns `null`.

use std::collections::BTreeMap;

use crate::model::{FlagSet, Metadata};

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
        let _ = (flag_key, context);
        todo!()
    }
}
