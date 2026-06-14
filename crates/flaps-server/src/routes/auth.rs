//! Public authentication routes (no auth required).

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{
    error::ApiError,
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
pub async fn post_login<S: Store>(
    State(state): State<AppState<S>>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
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
