//! Admin handlers for SDK key management within a project/environment scope.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use flaps_domain::{EnvironmentKey, ProjectKey, SdkKeyKind};
use flaps_store::{NewSdkKey, SdkKeyRecord, SdkKeyScope};
use serde::{Deserialize, Serialize};

use crate::{
    auth::AdminPrincipal,
    error::ApiError,
    state::{AppState, Store},
};

/// Path parameters shared by all SDK key routes.
type SdkKeyPath = (String, String);

/// Path parameters for single-key routes.
type SdkKeyItemPath = (String, String, String);

/// Request body for `POST .../keys`.
#[derive(Debug, Deserialize)]
pub struct CreateSdkKeyRequest {
    /// Server or client SDK kind.
    pub kind: SdkKeyKind,
}

/// Response body for `POST .../keys` (includes the raw secret, returned once).
#[derive(Debug, Serialize)]
pub struct CreateSdkKeyResponse {
    /// The raw SDK key. Only returned on creation; store never exposes it again.
    pub secret: String,
    /// Secret-free persisted record.
    pub record: SdkKeyRecord,
}

/// `POST /projects/:project/environments/:env/keys`
pub async fn post_sdk_key<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path((project_str, env_str)): Path<SdkKeyPath>,
    Json(body): Json<CreateSdkKeyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let project = ProjectKey::new(project_str).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let environment =
        EnvironmentKey::new(env_str).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    // Generate a raw key: prefix (kind letter) + 24 random bytes as hex.
    let raw_key = generate_sdk_key(body.kind);

    let new_key = NewSdkKey {
        kind: body.kind,
        scope: SdkKeyScope {
            project_key: project,
            environment_key: environment,
        },
    };

    let record = state
        .store
        .create_sdk_key(&raw_key, &new_key)
        .await
        .map_err(ApiError::from)?;

    let _ = principal; // actor auditing is done inside the store

    Ok((
        StatusCode::CREATED,
        Json(CreateSdkKeyResponse {
            secret: raw_key,
            record,
        }),
    ))
}

/// `GET /projects/:project/environments/:env/keys`
pub async fn list_sdk_keys<S: Store>(
    State(state): State<AppState<S>>,
    _principal: AdminPrincipal,
    Path((project_str, env_str)): Path<SdkKeyPath>,
) -> Result<Json<Vec<SdkKeyRecord>>, ApiError> {
    let project = ProjectKey::new(project_str).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let environment =
        EnvironmentKey::new(env_str).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    let scope = SdkKeyScope {
        project_key: project,
        environment_key: environment,
    };

    let records = state
        .store
        .list_sdk_keys("", &scope)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(records))
}

/// `DELETE /projects/:project/environments/:env/keys/:prefix`
pub async fn delete_sdk_key<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path((project_str, env_str, prefix)): Path<SdkKeyItemPath>,
) -> Result<StatusCode, ApiError> {
    let project = ProjectKey::new(project_str).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let environment =
        EnvironmentKey::new(env_str).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    state
        .store
        .revoke_sdk_key(&principal.username, &project, &environment, &prefix)
        .await
        .map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Generates a raw SDK key with a kind-specific prefix string.
fn generate_sdk_key(kind: SdkKeyKind) -> String {
    let prefix = match kind {
        SdkKeyKind::Server => "sv",
        SdkKeyKind::Client => "cl",
    };
    let mut bytes = [0u8; 24];
    OsRng.fill_bytes(&mut bytes);
    let hex: String = bytes.iter().fold(String::with_capacity(48), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    });
    format!("{prefix}_{hex}")
}
