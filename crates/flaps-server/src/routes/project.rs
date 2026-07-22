//! Admin handlers for the Project aggregate.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use flaps_domain::{ManagedBy, Project, ProjectKey};

use crate::{
    auth::AdminPrincipal,
    error::ApiError,
    etag::{check_if_match, check_if_none_match, compute_etag},
    recompile::{Change, evict_project_from_cache, install_in_cache, validate_by_compiling},
    state::{AppState, Store},
};

/// `GET /projects` -- list all projects.
pub async fn list_projects<S: Store>(
    State(state): State<AppState<S>>,
    _principal: AdminPrincipal,
) -> Result<Json<Vec<Project>>, ApiError> {
    let projects = state.store.list_projects().await.map_err(ApiError::from)?;
    Ok(Json(projects))
}

/// `GET /projects/{project}` -- fetch a single project with ETag.
pub async fn get_project<S: Store>(
    State(state): State<AppState<S>>,
    _principal: AdminPrincipal,
    Path(key): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let project_key = ProjectKey::new(key).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let project = state
        .store
        .get_project(&project_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let etag = compute_etag(&project)?;
    let mut response = Json(project).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag).map_err(|e| ApiError::Internal(e.to_string()))?,
    );
    Ok(response)
}

/// `PUT /projects/{project}` -- upsert a project (idempotent).
pub async fn put_project<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path(key): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Project>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal.username;
    let project_key = ProjectKey::new(key).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    // Path key must match body key.
    if project_key != body.key {
        return Err(ApiError::InvalidBody(
            "Path key does not match body key".to_owned(),
        ));
    }

    // Check If-Match / If-None-Match preconditions (both optional).
    let existing = state
        .store
        .get_project(&project_key)
        .await
        .map_err(ApiError::from)?;
    let is_create = existing.is_none();

    let current_etag = existing.as_ref().map(compute_etag).transpose()?;
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    check_if_match(if_match, current_etag.as_deref())?;

    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());
    check_if_none_match(if_none_match, existing.is_some())?;

    // Compile-as-validation.
    let rulesets = validate_by_compiling(&state, &project_key, &Change::UpsertProject).await?;

    // Write.
    state
        .store
        .upsert_project(&actor, &body)
        .await
        .map_err(ApiError::from)?;

    // Update cache.
    install_in_cache(&state, &project_key, rulesets).await;

    // Build response.
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

/// `DELETE /projects/{project}` -- delete a project.
pub async fn delete_project<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal.username;
    let project_key = ProjectKey::new(key).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    let existing = state
        .store
        .get_project(&project_key)
        .await
        .map_err(ApiError::from)?;

    // Check If-Match before the existence check: a specific-ETag or `*`
    // precondition on a missing resource is a 412, not a 404 (RFC 7232 SS3.1).
    let current_etag = existing.as_ref().map(compute_etag).transpose()?;
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    check_if_match(if_match, current_etag.as_deref())?;

    if existing.is_none() {
        return Err(ApiError::NotFound);
    }

    // validate_by_compiling for DeleteProject returns empty (no compilation needed).
    validate_by_compiling(&state, &project_key, &Change::DeleteProject).await?;

    // Write.
    state
        .store
        .delete_project(&actor, &project_key)
        .await
        .map_err(ApiError::from)?;

    // Evict all (project, *) entries from cache.
    evict_project_from_cache(&state, &project_key).await;

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
