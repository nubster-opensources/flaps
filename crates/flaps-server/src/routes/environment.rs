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
    etag::{check_if_match, check_if_none_match, compute_etag, read_precondition_header},
    recompile::{Change, evict_environment_from_cache, recompile_committed, validate_by_compiling},
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

    // Hold the per-project lock for the whole cycle (issues #105, #108).
    let lock = state.lock_project(&project_key).await;

    // The parent project must exist. Checking explicitly up front (rather than
    // relying on the foreign-key violation the write would eventually raise)
    // gives a clean 404 without compiling an empty ruleset for a new environment.
    let parent_exists = state
        .store
        .get_project(&project_key)
        .await
        .map_err(ApiError::from)?
        .is_some();
    if !parent_exists {
        // Release the registry entry: otherwise every distinct never-created
        // project key ever mentioned in a PUT would permanently occupy one.
        drop(lock);
        state.release_project_lock_if_unused(&project_key);
        return Err(ApiError::NotFound);
    }

    let existing = state
        .store
        .get_environment(&project_key, &env_key)
        .await
        .map_err(ApiError::from)?;
    let is_create = existing.is_none();

    let current_etag = existing.as_ref().map(compute_etag).transpose()?;
    let if_match = read_precondition_header(&headers, &header::IF_MATCH)?;
    check_if_match(if_match.as_deref(), current_etag.as_deref())?;

    let if_none_match = read_precondition_header(&headers, &header::IF_NONE_MATCH)?;
    check_if_none_match(if_none_match.as_deref(), existing.is_some())?;

    // Compile-as-validation: for a new environment there are no configs yet, which
    // means the compile succeeds with an empty flag set (valid).
    let rulesets =
        validate_by_compiling(&state, &project_key, &Change::UpsertEnvironment(&body)).await?;
    let affected: Vec<_> = rulesets.into_iter().map(|r| r.environment).collect();

    state
        .store
        .upsert_environment(&actor, &project_key, &body)
        .await
        .map_err(ApiError::from)?;

    recompile_committed(&state, &project_key, &affected).await;

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

    let lock = state.lock_project(&project_key).await;

    let existing = state
        .store
        .get_environment(&project_key, &env_key)
        .await
        .map_err(ApiError::from)?;

    let current_etag = existing.as_ref().map(compute_etag).transpose()?;
    let if_match = read_precondition_header(&headers, &header::IF_MATCH)?;
    check_if_match(if_match.as_deref(), current_etag.as_deref())?;

    if existing.is_none() {
        // The environment (and, most likely, its parent project) does not
        // exist: release the registry entry so repeated requests against a
        // never-created project key do not leak one entry each.
        drop(lock);
        state.release_project_lock_if_unused(&project_key);
        return Err(ApiError::NotFound);
    }

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
