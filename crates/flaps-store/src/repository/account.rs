//! Repository trait for local admin accounts.

use std::{future::Future, time::Duration};

use crate::{account::AccountRecord, error::StoreResult};

/// Async operations for creating and verifying local admin accounts.
pub trait AccountRepository: Send + Sync {
    /// Creates a new account with `username` and `password` (argon2id-hashed).
    ///
    /// Returns a [`StoreError::Conflict`] if the username is already taken.
    fn create_account(
        &self,
        actor: &str,
        username: &str,
        password: &str,
    ) -> impl Future<Output = StoreResult<AccountRecord>> + Send;

    /// Verifies `username` and `password` against the stored hash.
    ///
    /// Returns `None` if the account is unknown, inactive, or the password is
    /// wrong. The three cases are indistinguishable (anti-enumeration).
    fn verify_credentials(
        &self,
        username: &str,
        password: &str,
    ) -> impl Future<Output = StoreResult<Option<AccountRecord>>> + Send;
}

/// Async operations for session lifecycle management.
pub trait SessionRepository: Send + Sync {
    /// Mints a session for `account_id` valid for `ttl`.
    ///
    /// Returns the raw bearer token (clear, one time only) and the expiration.
    fn create_session(
        &self,
        account_id: &str,
        ttl: Duration,
    ) -> impl Future<Output = StoreResult<crate::account::NewSession>> + Send;

    /// Resolves a raw bearer token to an [`AccountRecord`].
    ///
    /// Returns `None` if the session is unknown, revoked, or expired.
    fn resolve_session(
        &self,
        raw_token: &str,
    ) -> impl Future<Output = StoreResult<Option<AccountRecord>>> + Send;

    /// Soft-revokes the session bound to `raw_token`. No-op if unknown.
    fn revoke_session(&self, raw_token: &str) -> impl Future<Output = StoreResult<()>> + Send;
}
