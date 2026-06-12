//! Compilation errors emitted by [`crate::compile_environment`].

/// All errors that can occur while compiling a flag set for one environment.
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// A flag references a segment key that was not provided to the compiler.
    #[error("flag `{flag}` references unknown segment `{segment}`")]
    UnknownSegment {
        /// Key of the flag that owns the rule.
        flag: String,
        /// Segment key that could not be resolved.
        segment: String,
    },

    /// A serve target names a variant key not declared in the flag's variant set.
    #[error("serve target in flag `{flag}` references unknown variant `{variant}`")]
    UnknownVariant {
        /// Key of the flag that owns the rule.
        flag: String,
        /// Variant key that could not be resolved.
        variant: String,
    },

    /// A flag typed as `Object` contains a variant whose JSON value is not an object.
    #[error("object variant `{variant}` in flag `{flag}` is not a JSON object")]
    ObjectVariantNotObject {
        /// Key of the flag.
        flag: String,
        /// Variant key whose value is not a JSON object.
        variant: String,
    },

    /// A predicate uses an operator with the wrong number of values.
    #[error("operator `{operator}` expects {expected} value(s), got {got}")]
    PredicateArity {
        /// Name of the operator.
        operator: String,
        /// Human-readable description of the expected arity.
        expected: String,
        /// Actual number of values provided.
        got: usize,
    },

    /// A scalar operator received a non-scalar JSON value (array or object).
    #[error("operator `{operator}` requires a scalar value")]
    NonScalarPredicateValue {
        /// Name of the operator.
        operator: String,
    },

    /// The compiled document was rejected by the `flaps-eval` parser.
    ///
    /// This indicates an internal compiler bug; the produced document is not
    /// a valid flagd ruleset.
    #[error("compiled ruleset for `{environment}` was rejected by the evaluator: {reason}")]
    EvaluatorRejected {
        /// Environment whose compilation produced an invalid document.
        environment: String,
        /// Underlying parse error message.
        reason: String,
    },
}
