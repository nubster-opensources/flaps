//! Public authentication routes (no auth required).

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{
    error::ApiError,
    preauth::limits::validate_credential_lengths,
    state::{AppState, Store},
};

/// Request body for `POST /login`.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    /// Account username.
    pub username: String,
    /// Plain-text password (hashed by the store).
    pub password: String,
}

/// Successful response body for `POST /login`.
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    /// Opaque bearer token to use in subsequent `Authorization: Bearer` headers.
    pub token: String,
    /// ISO-8601 UTC expiration timestamp.
    pub expires_at: String,
}

/// `POST /login` - verify credentials and mint a session token.
///
/// # Errors
/// - 422 unprocessable entity when a credential exceeds
///   [`MAX_USERNAME_BYTES`](crate::preauth::limits::MAX_USERNAME_BYTES) or
///   [`MAX_PASSWORD_BYTES`](crate::preauth::limits::MAX_PASSWORD_BYTES).
/// - 429 too many requests (`Retry-After` header), throttled per username via
///   [`AppState::login_rate_limiter`], before credentials are even checked.
pub async fn post_login<S: Store>(
    State(state): State<AppState<S>>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    // Bound what the caller sent before anything allocates on its behalf.
    validate_credential_lengths(&body.username, &body.password)?;

    // Rate limit keyed by username, ahead of any store access: caps the rate
    // of brute-force attempts against a single account.
    state
        .login_rate_limiter
        .check(&body.username)
        .map_err(|retry_after_seconds| ApiError::TooManyRequests {
            retry_after_seconds,
        })?;

    let account = state
        .store
        .verify_credentials(&body.username, &body.password)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::Unauthorized)?;

    let session = state
        .store
        .create_session(&account.id, state.session_ttl)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(LoginResponse {
        token: session.token,
        expires_at: session.expires_at,
    }))
}
