//! Admin handlers for the `FlagEnvConfig` aggregate.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use flaps_domain::{EnvironmentKey, FlagEnvConfig, FlagKey, ProjectKey};

use crate::{
    auth::AdminPrincipal,
    error::ApiError,
    etag::{check_if_match, compute_etag},
    recompile::{Change, install_in_cache, validate_by_compiling},
    state::{AppState, Store},
};

/// `GET /projects/{project}/flags/{flag}/environments/{env}/config`
pub async fn get_flag_env_config<S: Store>(
    State(state): State<AppState<S>>,
    _principal: AdminPrincipal,
    Path((project, flag, env)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let flag_key = FlagKey::new(flag).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let env_key = EnvironmentKey::new(env).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    let config = state
        .store
        .get_flag_env_config(&project_key, &flag_key, &env_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let etag = compute_etag(&config)?;
    let mut response = Json(config).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag).map_err(|e| ApiError::Internal(e.to_string()))?,
    );
    Ok(response)
}

/// `PUT /projects/{project}/flags/{flag}/environments/{env}/config` -- upsert a config.
pub async fn put_flag_env_config<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path((project, flag, env)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<FlagEnvConfig>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal.username;
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let flag_key = FlagKey::new(flag).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let env_key = EnvironmentKey::new(env).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    // All three parents must exist. Checking explicitly up front (rather than
    // relying on the foreign-key violation the write would eventually raise)
    // gives a clean 404 without compiling an environment for a non-existent flag.
    state
        .store
        .get_project(&project_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;
    state
        .store
        .get_flag(&project_key, &flag_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;
    state
        .store
        .get_environment(&project_key, &env_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let existing = state
        .store
        .get_flag_env_config(&project_key, &flag_key, &env_key)
        .await
        .map_err(ApiError::from)?;
    let is_create = existing.is_none();

    if let Some(ref current) = existing {
        let current_etag = compute_etag(current)?;
        let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
        check_if_match(if_match, &current_etag)?;
    }

    // Compile-as-validation: compile the affected environment with the new config.
    let change = Change::UpsertFlagEnvConfig {
        flag: &flag_key,
        environment: &env_key,
        config: &body,
    };
    let rulesets = validate_by_compiling(&state, &project_key, &change).await?;

    state
        .store
        .upsert_flag_env_config(&actor, &project_key, &flag_key, &env_key, &body)
        .await
        .map_err(ApiError::from)?;

    install_in_cache(&state, &project_key, rulesets).await;

    let etag = compute_etag(&body)?;
    let status = if is_create {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let mut response = response_with_body(status, &body)?;
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag).map_err(|e| ApiError::Internal(e.to_string()))?,
    );
    Ok(response)
}

/// `DELETE /projects/{project}/flags/{flag}/environments/{env}/config` -- delete a config.
pub async fn delete_flag_env_config<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path((project, flag, env)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal.username;
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let flag_key = FlagKey::new(flag).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let env_key = EnvironmentKey::new(env).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    let existing = state
        .store
        .get_flag_env_config(&project_key, &flag_key, &env_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let current_etag = compute_etag(&existing)?;
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    check_if_match(if_match, &current_etag)?;

    let change = Change::DeleteFlagEnvConfig {
        flag: &flag_key,
        environment: &env_key,
    };
    let rulesets = validate_by_compiling(&state, &project_key, &change).await?;

    state
        .store
        .delete_flag_env_config(&actor, &project_key, &flag_key, &env_key)
        .await
        .map_err(ApiError::from)?;

    install_in_cache(&state, &project_key, rulesets).await;

    Ok(StatusCode::NO_CONTENT)
}

fn response_with_body<T: serde::Serialize>(
    status: StatusCode,
    body: &T,
) -> Result<Response, ApiError> {
    let bytes = serde_json::to_vec(body)
        .map_err(|e| ApiError::Internal(format!("Response serialization failed: {e}")))?;
    Ok(Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()))
}
