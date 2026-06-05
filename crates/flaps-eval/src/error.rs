//! Structured errors produced while parsing flagd documents.

/// An error encountered while parsing a flagd flag set document.
///
/// Every variant carries enough context (JSON path, offending operator or
/// reference) to be actionable without re-reading the source document.
/// Parsing never panics: any malformed input maps to one of these variants.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The document is not syntactically valid JSON.
    #[error("invalid JSON document: {0}")]
    Json(#[from] serde_json::Error),

    /// The document structure does not match the flagd schema.
    #[error("invalid document structure at `{path}`: {reason}")]
    InvalidDocument {
        /// JSON path of the offending element.
        path: String,
        /// Human readable description of the mismatch.
        reason: String,
    },

    /// A targeting rule uses an operator unknown to the flagd format.
    #[error("unknown operator `{operator}` at `{path}`")]
    UnknownOperator {
        /// JSON path of the offending rule.
        path: String,
        /// The unrecognized operator name.
        operator: String,
    },

    /// A known operator received arguments of the wrong shape or type.
    #[error("invalid arguments for `{operator}` at `{path}`: {reason}")]
    InvalidArguments {
        /// JSON path of the offending rule.
        path: String,
        /// The operator whose arguments are malformed.
        operator: String,
        /// Human readable description of the mismatch.
        reason: String,
    },

    /// The variants of a flag mix more than one value type.
    #[error("variants of flag `{flag_key}` mix more than one value type")]
    MixedVariantTypes {
        /// Key of the offending flag.
        flag_key: String,
    },

    /// A `$ref` targets an evaluator that is not declared in `$evaluators`.
    #[error("unresolved evaluator reference `{reference}` at `{path}`")]
    UnknownEvaluator {
        /// JSON path of the offending reference.
        path: String,
        /// The missing evaluator name.
        reference: String,
    },

    /// Evaluator references form a cycle and cannot be inlined.
    #[error("evaluator reference cycle involving `{reference}`")]
    EvaluatorCycle {
        /// One evaluator participating in the cycle.
        reference: String,
    },
}
