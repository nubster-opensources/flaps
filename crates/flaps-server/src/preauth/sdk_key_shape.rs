//! Refusing bearer credentials that cannot possibly be a valid SDK key.
//!
//! This never proves a key is valid. It proves a key is impossible, which is
//! enough to refuse before touching the database, and that is the whole point:
//! the SDK rate limiter is keyed on a prefix known only after the lookup, so an
//! invalid key is never throttled and costs one query per HTTP request today.

use crate::error::ApiError;

/// Accepted key prefixes, one per SDK kind.
const ACCEPTED_PREFIXES: [&str; 2] = ["sv_", "cl_"];

/// Length in bytes of the hexadecimal body.
const BODY_HEX_CHARS: usize = 48;

/// Total accepted length of a raw SDK key, in bytes.
///
/// Three bytes of prefix plus forty-eight hexadecimal characters: exactly what
/// the key generator emits.
pub const SDK_KEY_TOTAL_BYTES: usize = 3 + BODY_HEX_CHARS;

/// Rejects a bearer credential that cannot possibly be a valid SDK key.
///
/// Checks prefix, encoding and length only.
///
/// # Errors
/// Returns [`ApiError::Unauthorized`], the same outcome an unknown key
/// produces. An impossible key, an absent key and a throttled key must be
/// observationally identical, otherwise the status code becomes an oracle of
/// key validity. The error never carries the submitted material.
pub fn reject_impossible_sdk_key(raw: &str) -> Result<(), ApiError> {
    if raw.len() != SDK_KEY_TOTAL_BYTES {
        return Err(ApiError::Unauthorized);
    }

    let Some(body) = ACCEPTED_PREFIXES
        .iter()
        .find_map(|prefix| raw.strip_prefix(prefix))
    else {
        return Err(ApiError::Unauthorized);
    };

    if body.len() != BODY_HEX_CHARS
        || !body
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ApiError::Unauthorized);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn well_formed(prefix: &str) -> String {
        format!("{prefix}_{}", "a1".repeat(24))
    }

    #[test]
    fn a_well_formed_server_key_is_accepted() {
        assert!(reject_impossible_sdk_key(&well_formed("sv")).is_ok());
    }

    #[test]
    fn a_well_formed_client_key_is_accepted() {
        assert!(reject_impossible_sdk_key(&well_formed("cl")).is_ok());
    }

    #[test]
    fn the_accepted_length_matches_what_the_server_generates() {
        assert_eq!(well_formed("sv").len(), SDK_KEY_TOTAL_BYTES);
    }

    #[test]
    fn an_unknown_prefix_is_refused() {
        assert!(reject_impossible_sdk_key(&well_formed("xx")).is_err());
    }

    #[test]
    fn a_short_credential_is_refused() {
        assert!(reject_impossible_sdk_key("sv_deadbeef").is_err());
    }

    #[test]
    fn an_oversized_credential_is_refused() {
        assert!(reject_impossible_sdk_key(&"z".repeat(64 * 1024)).is_err());
    }

    #[test]
    fn a_non_hexadecimal_body_is_refused() {
        let raw = format!("sv_{}", "g".repeat(48));
        assert!(reject_impossible_sdk_key(&raw).is_err());
    }

    #[test]
    fn uppercase_hexadecimal_is_refused() {
        // The generator emits lowercase only. Accepting both would let one key
        // occupy two distinct rate-limiter buckets.
        let raw = format!("sv_{}", "A1".repeat(24));
        assert!(reject_impossible_sdk_key(&raw).is_err());
    }

    #[test]
    fn the_refusal_never_echoes_the_credential() {
        // 47 characters of body: well formed in every respect except length,
        // so the refusal comes from the shape check and not from the alphabet.
        let raw = format!("sv_{}deadbee", "deadbeef".repeat(5));
        assert_eq!(raw.len(), SDK_KEY_TOTAL_BYTES - 1);

        let Err(error) = reject_impossible_sdk_key(&raw) else {
            panic!("this credential must be refused");
        };
        assert!(
            !format!("{error:?}").contains("deadbee"),
            "an error must never carry key material into a log"
        );
    }
}
