//! Persisted, secret-free forms of SDK keys.

use flaps_domain::{EnvironmentKey, ProjectKey, SdkKeyKind};
use serde::Serialize;

/// Project and environment scope an SDK key is bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SdkKeyScope {
    /// The project this key belongs to.
    pub project_key: ProjectKey,
    /// The environment this key is scoped to.
    pub environment_key: EnvironmentKey,
}

/// Input required to create a new SDK key.
///
/// The raw key value is passed separately to the repository method so it is
/// never stored on this struct.
#[derive(Debug, Clone)]
pub struct NewSdkKey {
    /// Server or client SDK kind.
    pub kind: SdkKeyKind,
    /// Project and environment scope.
    pub scope: SdkKeyScope,
}

/// Persisted, secret-free view of an SDK key.
///
/// Never carries the raw value nor the HMAC hash; exposes only the readable
/// prefix so callers can identify a key without being able to reuse it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SdkKeyRecord {
    /// The leading readable characters of the original raw key.
    pub prefix: String,
    /// Server or client SDK kind.
    pub kind: SdkKeyKind,
    /// Project and environment scope.
    pub scope: SdkKeyScope,
    /// ISO-8601 UTC creation timestamp.
    pub created_at: String,
}
