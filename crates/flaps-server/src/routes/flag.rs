//! Admin handlers for the Flag aggregate.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use flaps_domain::{Flag, FlagKey, ProjectKey};

use crate::{
    auth::AdminPrincipal,
    error::ApiError,
    etag::{check_if_match, compute_etag},
    recompile::{Change, install_in_cache, validate_by_compiling},
    state::{AppState, Store},
};

/// `GET /projects/{project}/flags` -- list all flags in a project.
pub async fn list_flags<S: Store>(
    State(state): State<AppState<S>>,
    _principal: AdminPrincipal,
    Path(project): Path<String>,
) -> Result<Json<Vec<Flag>>, ApiError> {
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let flags = state
        .store
        .list_flags(&project_key)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(flags))
}

/// `GET /projects/{project}/flags/{flag}` -- fetch a single flag with ETag.
pub async fn get_flag<S: Store>(
    State(state): State<AppState<S>>,
    _principal: AdminPrincipal,
    Path((project, flag)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let flag_key = FlagKey::new(flag).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    let flag = state
        .store
        .get_flag(&project_key, &flag_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let etag = compute_etag(&flag)?;
    let mut response = Json(flag).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag).map_err(|e| ApiError::Internal(e.to_string()))?,
    );
    Ok(response)
}

/// `PUT /projects/{project}/flags/{flag}` -- upsert a flag.
pub async fn put_flag<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path((project, flag)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Flag>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal.username;
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let flag_key = FlagKey::new(flag).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    if flag_key != body.key {
        return Err(ApiError::InvalidBody(
            "Path key does not match body key".to_owned(),
        ));
    }

    let existing = state
        .store
        .get_flag(&project_key, &flag_key)
        .await
        .map_err(ApiError::from)?;
    let is_create = existing.is_none();

    if let Some(ref current) = existing {
        let current_etag = compute_etag(current)?;
        let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
        check_if_match(if_match, &current_etag)?;
    }

    // Compile-as-validation.
    let rulesets = validate_by_compiling(&state, &project_key, &Change::UpsertFlag(&body)).await?;

    state
        .store
        .upsert_flag(&actor, &project_key, &body)
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

/// `DELETE /projects/{project}/flags/{flag}` -- delete a flag.
pub async fn delete_flag<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path((project, flag)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal.username;
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let flag_key = FlagKey::new(flag).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    let existing = state
        .store
        .get_flag(&project_key, &flag_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let current_etag = compute_etag(&existing)?;
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    check_if_match(if_match, &current_etag)?;

    // Compile affected envs without this flag to ensure nothing breaks.
    let rulesets =
        validate_by_compiling(&state, &project_key, &Change::DeleteFlag(&flag_key)).await?;

    state
        .store
        .delete_flag(&actor, &project_key, &flag_key)
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
