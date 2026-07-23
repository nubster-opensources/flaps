//! Byte-length bounds accepted on unauthenticated credentials.
//!
//! These bounds are deliberately not configurable. A security bound an
//! operator can raise without measuring the consequence is not a bound.

use crate::error::ApiError;

/// Maximum accepted byte length of a submitted username.
pub const MAX_USERNAME_BYTES: usize = 256;

/// Maximum accepted byte length of a submitted password.
pub const MAX_PASSWORD_BYTES: usize = 1024;

/// Maximum accepted body size of a login request, in bytes.
///
/// Applied as an axum body limit on the login route alone. The framework
/// default of two mebibytes is meaningless for a credential pair.
pub const MAX_LOGIN_BODY_BYTES: usize = 4 * 1024;

/// Rejects credentials whose byte length exceeds the accepted bounds.
///
/// Validation happens before the rate limiter, before any store access and
/// before any password hashing, so an oversized credential costs a length
/// comparison rather than a bucket allocation and an Argon2 verification.
///
/// # Errors
/// Returns [`ApiError::InvalidBody`] when either credential is too long. The
/// message names the field but never echoes the submitted value.
pub fn validate_credential_lengths(username: &str, password: &str) -> Result<(), ApiError> {
    if username.len() > MAX_USERNAME_BYTES {
        return Err(ApiError::InvalidBody(format!(
            "username exceeds the accepted length of {MAX_USERNAME_BYTES} bytes"
        )));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(ApiError::InvalidBody(format!(
            "password exceeds the accepted length of {MAX_PASSWORD_BYTES} bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_within_bounds_are_accepted() {
        assert!(validate_credential_lengths("admin", "password").is_ok());
    }

    #[test]
    fn a_username_at_the_bound_is_accepted() {
        let username = "a".repeat(MAX_USERNAME_BYTES);
        assert!(validate_credential_lengths(&username, "password").is_ok());
    }

    #[test]
    fn a_username_one_byte_over_the_bound_is_refused() {
        let username = "a".repeat(MAX_USERNAME_BYTES + 1);
        assert!(validate_credential_lengths(&username, "password").is_err());
    }

    #[test]
    fn a_password_one_byte_over_the_bound_is_refused() {
        let password = "b".repeat(MAX_PASSWORD_BYTES + 1);
        assert!(validate_credential_lengths("admin", &password).is_err());
    }

    #[test]
    fn the_bound_counts_bytes_not_characters() {
        // Each of these characters costs two bytes in UTF-8.
        let username = "e\u{301}".repeat(MAX_USERNAME_BYTES);
        assert!(
            validate_credential_lengths(&username, "password").is_err(),
            "the bound must be measured in bytes, since bytes are what is allocated"
        );
    }
}
