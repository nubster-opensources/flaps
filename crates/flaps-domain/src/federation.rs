//! Federation markers for domain aggregates.
//!
//! [`ExternalRef`] is an opaque handle to a resource managed outside Flaps.
//! It is never parsed or interpreted by the domain.

use serde::{Deserialize, Serialize};

/// An opaque reference to an external resource.
///
/// The domain carries this value unchanged across the API boundary. It is
/// never interpreted, parsed, or validated beyond round-trip fidelity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExternalRef(String);

impl ExternalRef {
    /// Wraps `value` as an opaque external reference.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the raw reference string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Declares whether an aggregate is owned locally or by a federated source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedBy {
    /// Owned and mutated by this Flaps instance.
    Local,
    /// Read-only replica pushed from an external federation source.
    Federated,
}
