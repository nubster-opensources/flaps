//! Admin handlers for the Environment aggregate.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use flaps_domain::{Environment, EnvironmentKey, ManagedBy, ProjectKey};

use crate::{
    auth::AdminPrincipal,
    error::ApiError,
    etag::{check_if_match, compute_etag},
    recompile::{Change, evict_environment_from_cache, install_in_cache, validate_by_compiling},
    state::{AppState, Store},
};

/// `GET /projects/{project}/environments` -- list environments in a project.
pub async fn list_environments<S: Store>(
    State(state): State<AppState<S>>,
    _principal: AdminPrincipal,
    Path(project): Path<String>,
) -> Result<Json<Vec<Environment>>, ApiError> {
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let envs = state
        .store
        .list_environments(&project_key)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(envs))
}

/// `GET /projects/{project}/environments/{env}` -- fetch a single environment with ETag.
pub async fn get_environment<S: Store>(
    State(state): State<AppState<S>>,
    _principal: AdminPrincipal,
    Path((project, env)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let env_key = EnvironmentKey::new(env).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    let environment = state
        .store
        .get_environment(&project_key, &env_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let etag = compute_etag(&environment)?;
    let mut response = Json(environment).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag).map_err(|e| ApiError::Internal(e.to_string()))?,
    );
    Ok(response)
}

/// `PUT /projects/{project}/environments/{env}` -- upsert an environment.
pub async fn put_environment<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path((project, env)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Environment>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal.username;
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let env_key = EnvironmentKey::new(env).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    if env_key != body.key {
        return Err(ApiError::InvalidBody(
            "Path key does not match body key".to_owned(),
        ));
    }

    // The parent project must exist. Checking explicitly up front (rather than
    // relying on the foreign-key violation the write would eventually raise)
    // gives a clean 404 without compiling an empty ruleset for a new environment.
    state
        .store
        .get_project(&project_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let existing = state
        .store
        .get_environment(&project_key, &env_key)
        .await
        .map_err(ApiError::from)?;
    let is_create = existing.is_none();

    if let Some(ref current) = existing {
        let current_etag = compute_etag(current)?;
        let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
        check_if_match(if_match, &current_etag)?;
    }

    // Compile-as-validation: for a new environment there are no configs yet, which
    // means the compile succeeds with an empty flag set (valid).
    let rulesets =
        validate_by_compiling(&state, &project_key, &Change::UpsertEnvironment(&body)).await?;

    state
        .store
        .upsert_environment(&actor, &project_key, &body)
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
    if body.managed_by == ManagedBy::Federated {
        response.headers_mut().insert(
            "X-Flaps-Warning",
            HeaderValue::from_static(
                "This resource is federated; local edits may be overwritten by the federation.",
            ),
        );
    }
    Ok(response)
}

/// `DELETE /projects/{project}/environments/{env}` -- delete an environment.
pub async fn delete_environment<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path((project, env)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal.username;
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let env_key = EnvironmentKey::new(env).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    let existing = state
        .store
        .get_environment(&project_key, &env_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let current_etag = compute_etag(&existing)?;
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    check_if_match(if_match, &current_etag)?;

    // No compilation needed for delete-environment.
    validate_by_compiling(&state, &project_key, &Change::DeleteEnvironment(&env_key)).await?;

    state
        .store
        .delete_environment(&actor, &project_key, &env_key)
        .await
        .map_err(ApiError::from)?;

    evict_environment_from_cache(&state, &project_key, &env_key).await;

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
