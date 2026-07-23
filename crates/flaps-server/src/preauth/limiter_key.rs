//! Fixed-size limiter keys derived from attacker-controlled input.
//!
//! Two distinct problems are solved by keying the derivation under a secret
//! generated at process start. Without a secret, an attacker can precompute
//! inputs that land in the same bucket as a target account and have it
//! throttled in their place. And the bucket table would otherwise be a
//! readable directory of every identifier that has been tried.

use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::Sha256;

/// Length in bytes of the derived key.
///
/// Sixteen bytes leave collisions far out of reach for a table bounded at a
/// hundred thousand buckets, while keeping the map entry small.
const DERIVED_KEY_BYTES: usize = 16;

/// Length in bytes of the per-process derivation secret.
const SECRET_BYTES: usize = 32;

/// A fixed-size limiter key derived from attacker-controlled input.
///
/// The bucket map is keyed by this type rather than by the raw identifier, so
/// the memory a single bucket costs no longer depends on the length of what
/// the caller sent.
///
/// The [`std::fmt::Debug`] rendering shows the derived bytes only: the
/// identifier it came from is never recoverable from a log line or a memory
/// dump of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LimiterKey([u8; DERIVED_KEY_BYTES]);

/// Derives limiter keys under a per-process secret.
pub struct LimiterKeyDeriver {
    secret: [u8; SECRET_BYTES],
}

impl LimiterKeyDeriver {
    /// Builds a deriver with a freshly generated per-process secret.
    ///
    /// The secret never leaves the process and is never persisted: it only
    /// has to be unpredictable to whoever is sending requests right now.
    #[must_use]
    pub fn new() -> Self {
        use argon2::password_hash::rand_core::{OsRng, RngCore as _};

        let mut secret = [0u8; SECRET_BYTES];
        OsRng.fill_bytes(&mut secret);
        Self { secret }
    }

    /// Derives the fixed-size key for an arbitrary identifier.
    #[must_use]
    pub fn derive(&self, identifier: &str) -> LimiterKey {
        #[allow(clippy::expect_used)]
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC accepts a key of any length");
        mac.update(identifier.as_bytes());
        let tag = mac.finalize().into_bytes();

        let mut derived = [0u8; DERIVED_KEY_BYTES];
        derived.copy_from_slice(&tag[..DERIVED_KEY_BYTES]);
        LimiterKey(derived)
    }
}

impl Default for LimiterKeyDeriver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_identifier_derives_the_same_key() {
        let deriver = LimiterKeyDeriver::new();
        assert_eq!(deriver.derive("alice"), deriver.derive("alice"));
    }

    #[test]
    fn distinct_identifiers_derive_distinct_keys() {
        let deriver = LimiterKeyDeriver::new();
        assert_ne!(deriver.derive("alice"), deriver.derive("bob"));
    }

    #[test]
    fn two_derivers_disagree_on_the_same_identifier() {
        // The per-process secret is what prevents an attacker from computing,
        // ahead of time, an input that lands in a target account's bucket.
        let first = LimiterKeyDeriver::new();
        let second = LimiterKeyDeriver::new();
        assert_ne!(first.derive("alice"), second.derive("alice"));
    }

    #[test]
    fn key_size_is_independent_of_identifier_length() {
        let deriver = LimiterKeyDeriver::new();
        let short = deriver.derive("a");
        let long = deriver.derive(&"a".repeat(4096));
        assert_eq!(
            std::mem::size_of_val(&short),
            std::mem::size_of_val(&long),
            "a bucket must never cost more because the caller sent more"
        );
    }

    #[test]
    fn the_debug_rendering_never_exposes_the_identifier() {
        let deriver = LimiterKeyDeriver::new();
        let rendered = format!("{:?}", deriver.derive("secret-account-name"));
        assert!(
            !rendered.contains("secret-account-name"),
            "the bucket table must not become a directory of attempted names"
        );
    }
}
