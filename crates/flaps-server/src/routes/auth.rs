//! Public authentication routes (no auth required).

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{
    error::ApiError,
    preauth::{
        budget::PreAuthRejection, client_address::ClientAddress,
        limits::validate_credential_lengths,
    },
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
/// # Ordering
/// Length bounds, then the layered pre-authentication budget, then the login
/// rate limiter, then the password verification concurrency ceiling, then the
/// store. Each step is cheaper than the one it guards.
///
/// # Errors
/// - 422 unprocessable entity when a credential exceeds its accepted length.
/// - 429 too many requests (`Retry-After` header) when the pre-authentication
///   budget is exhausted, before any store access.
/// - 401 unauthorized when the credentials do not match.
pub async fn post_login<S: Store>(
    State(state): State<AppState<S>>,
    client: ClientAddress,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    // Bound what the caller sent before anything allocates on its behalf.
    validate_credential_lengths(&body.username, &body.password)?;

    // Layered budget: the widest layer refuses first and costs the least.
    state.preauth_budget.consume(client)?;

    // Per-account throttle: the single component owning the per-identity
    // policy since the redundant budget layer was collapsed into it (#158).
    // Its refusal is mapped to the uniform pre-auth rejection, so a login
    // throttle is indistinguishable from a budget refusal and advertises the
    // same Retry-After; the token bucket's dynamic wait is deliberately
    // discarded, since it would leak the targeted account bucket's fill level.
    state
        .login_rate_limiter
        .check(&body.username)
        .map_err(|_| PreAuthRejection::IdentityBudgetExhausted)?;

    // Hold the permit across the entire verification so the number of Argon2
    // computations in flight is bounded by the pool, not by Tokio's blocking pool.
    let _verification_permit = state.password_pool.try_acquire().map_err(ApiError::from)?;

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
