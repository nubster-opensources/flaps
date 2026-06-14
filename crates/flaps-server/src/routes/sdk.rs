//! SDK endpoints (authenticated via SDK key, rate-limited).

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use flaps_domain::SdkKeyKind;
use flaps_store::SdkKeyScope;
use serde::Serialize;

use crate::{
    auth::SdkKeyPrincipal,
    error::ApiError,
    state::{AppState, Store},
};

/// Response body for `GET /sdk/whoami`.
#[derive(Debug, Serialize)]
pub struct WhoamiResponse {
    /// Project and environment scope this key is bound to.
    pub scope: SdkKeyScopeDto,
    /// Server or client SDK kind.
    pub kind: SdkKeyKind,
}

/// Serializable DTO for [`SdkKeyScope`].
#[derive(Debug, Serialize)]
pub struct SdkKeyScopeDto {
    /// Project key.
    pub project_key: String,
    /// Environment key.
    pub environment_key: String,
}

impl From<SdkKeyScope> for SdkKeyScopeDto {
    fn from(s: SdkKeyScope) -> Self {
        Self {
            project_key: s.project_key.as_str().to_owned(),
            environment_key: s.environment_key.as_str().to_owned(),
        }
    }
}

/// `GET /sdk/whoami` - identify the SDK key and check rate limit.
pub async fn get_whoami<S: Store>(
    State(state): State<AppState<S>>,
    principal: Result<SdkKeyPrincipal, (StatusCode, ApiError)>,
) -> Result<impl IntoResponse, ApiError> {
    let principal = principal.map_err(|(_, e)| e)?;

    // Apply rate limit keyed by SDK key prefix.
    state
        .rate_limiter
        .check(&principal.prefix)
        .map_err(|retry_after_seconds| ApiError::TooManyRequests {
            retry_after_seconds,
        })?;

    Ok(Json(WhoamiResponse {
        scope: principal.scope.into(),
        kind: principal.kind,
    }))
}
