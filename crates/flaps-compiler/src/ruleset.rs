//! Output type produced by [`crate::compile_environment`].

use flaps_domain::key::EnvironmentKey;

/// A compiled, content-hashed ruleset for a single environment.
///
/// The `document` field is the canonical flagd JSON string produced by
/// `FlagSet::to_json()`. The `content_hash` is the hex-encoded SHA-256 of
/// that string and can be used as an HTTP ETag. The `version` is a monotone
/// counter: it stays unchanged when recompiling an identical input so that
/// consumers can detect actual changes with a cheap integer comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRuleset {
    /// The environment this ruleset was compiled for.
    pub environment: EnvironmentKey,
    /// Canonical flagd JSON document.
    pub document: String,
    /// Hex-encoded SHA-256 of `document`, usable as an ETag.
    pub content_hash: String,
    /// Monotone version: unchanged when the hash is unchanged.
    pub version: u64,
}
