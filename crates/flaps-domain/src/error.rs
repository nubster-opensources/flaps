//! Domain-level error types for the Flaps flag platform.

use crate::variant::ValueType;

/// Errors produced by domain smart constructors and validation rules.
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    /// The supplied string is not valid kebab-case.
    #[error("invalid key `{0}`: must be kebab-case")]
    InvalidKey(String),

    /// A variant value does not match the flag's declared value type.
    #[error("variant `{variant}` value does not match flag value type `{value_type:?}`")]
    VariantTypeMismatch {
        /// Name of the offending variant.
        variant: String,
        /// The flag's declared value type.
        value_type: ValueType,
    },

    /// A [`Variants`](crate::variant::Variants) map was constructed with no entries.
    #[error("variant set is empty")]
    EmptyVariants,

    /// Rollout weights do not sum to a positive total.
    #[error("rollout weights must sum to a positive total")]
    InvalidRollout,
}
