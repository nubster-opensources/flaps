//! HMAC-SHA256 hashing for SDK keys at rest.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Computes the at-rest hash of an SDK key using a server-held pepper.
///
/// Deterministic (no per-row salt) so the hash can be indexed and looked up.
/// The pepper is held in memory only and never persisted.
#[derive(Clone)]
pub struct KeyHasher {
    pepper: Vec<u8>,
}

impl KeyHasher {
    /// Builds a hasher from the raw pepper bytes.
    #[must_use]
    pub fn new(pepper: impl Into<Vec<u8>>) -> Self {
        Self {
            pepper: pepper.into(),
        }
    }

    /// Returns the lowercase hex HMAC-SHA256 of `raw_key` under the pepper.
    #[must_use]
    pub fn hash(&self, raw_key: &str) -> String {
        // infallible: HMAC accepts any key length (the underlying `get_der_key`
        // pads or hashes the key to fit the block size and never errors).
        #[allow(clippy::expect_used)]
        let mut mac =
            HmacSha256::new_from_slice(&self.pepper).expect("HMAC accepts any key length");
        mac.update(raw_key.as_bytes());
        let result = mac.finalize();
        encode_hex(&result.into_bytes())
    }
}

impl std::fmt::Debug for KeyHasher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyHasher")
            .field("pepper", &"<redacted>")
            .finish()
    }
}

/// Encodes a byte slice as a lowercase hex string without extra dependencies.
fn encode_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

#[cfg(test)]
mod tests {
    use super::KeyHasher;

    /// RFC 4231, Test Case 2 (HMAC-SHA-256): official external oracle, not
    /// tied to this crate's implementation. Our API maps pepper = HMAC key,
    /// `raw_key` = HMAC message. Must stay green across the sha2/hmac bump:
    /// any drift here would mean HMAC-SHA256 output changed, which is not
    /// expected to ever happen for a stable standard.
    #[test]
    fn rfc4231_test_case_2() {
        let hasher = KeyHasher::new(b"Jefe".to_vec());
        assert_eq!(
            hasher.hash("what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    /// Golden value for a representative (pepper, key) pair from this crate's
    /// own domain, locking the exact output of the full `hash()` path.
    /// Guards against any cross-version drift in the at-rest hash of SDK
    /// keys and session tokens: this hex value must NEVER be updated to make
    /// a failing test pass after a dependency bump. If it changes, every SDK
    /// key and session already persisted becomes unlookupable.
    #[test]
    fn golden_hash_is_stable_across_dependency_bumps() {
        let hasher = KeyHasher::new(b"e2e-golden-pepper-32-bytes-long!!".to_vec());
        assert_eq!(
            hasher.hash("sv-0123456789ab"),
            "6c1975bb16ee888e861e8862ab6426fecc1ae900bc93f62549a3e7d71703177d"
        );
    }
}
