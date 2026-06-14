//! Persisted types for local accounts and sessions.

/// Persisted, secret-free view of a local admin account.
///
/// Never carries the password hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    /// Opaque unique identifier (`UUIDv4`).
    pub id: String,
    /// Human-readable login name (unique).
    pub username: String,
}

/// Returned once by [`crate::repository::SessionRepository::create_session`].
///
/// The raw token is returned in clear exactly once. The caller is responsible
/// for transmitting it to the client and never logging it.
pub struct NewSession {
    /// Raw bearer token (opaque, URL-safe). Never stored at rest.
    pub token: String,
    /// ISO-8601 UTC expiration timestamp.
    pub expires_at: String,
}

impl std::fmt::Debug for NewSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewSession")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}
