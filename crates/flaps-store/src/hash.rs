//! HMAC-SHA256 hashing for SDK keys at rest.

use hmac::{Hmac, Mac};
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
