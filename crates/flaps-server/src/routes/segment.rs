//! Admin handlers for the Segment aggregate.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use flaps_domain::{ProjectKey, Segment, SegmentKey};

use crate::{
    auth::AdminPrincipal,
    error::ApiError,
    etag::{check_if_match, compute_etag},
    recompile::{Change, install_in_cache, validate_by_compiling},
    state::{AppState, Store},
};

/// `GET /projects/{project}/segments` -- list all segments in a project.
pub async fn list_segments<S: Store>(
    State(state): State<AppState<S>>,
    Path(project): Path<String>,
) -> Result<Json<Vec<Segment>>, ApiError> {
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let segments = state
        .store
        .list_segments(&project_key)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(segments))
}

/// `GET /projects/{project}/segments/{segment}` -- fetch a single segment with ETag.
pub async fn get_segment<S: Store>(
    State(state): State<AppState<S>>,
    Path((project, segment)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let segment_key = SegmentKey::new(segment).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    let seg = state
        .store
        .get_segment(&project_key, &segment_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let etag = compute_etag(&seg)?;
    let mut response = Json(seg).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag).map_err(|e| ApiError::Internal(e.to_string()))?,
    );
    Ok(response)
}

/// `PUT /projects/{project}/segments/{segment}` -- upsert a segment.
pub async fn put_segment<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path((project, segment)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Segment>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal.username;
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let segment_key = SegmentKey::new(segment).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    if segment_key != body.key {
        return Err(ApiError::InvalidBody(
            "Path key does not match body key".to_owned(),
        ));
    }

    let existing = state
        .store
        .get_segment(&project_key, &segment_key)
        .await
        .map_err(ApiError::from)?;
    let is_create = existing.is_none();

    if let Some(ref current) = existing {
        let current_etag = compute_etag(current)?;
        let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
        check_if_match(if_match, &current_etag)?;
    }

    // Compile-as-validation: recompile all envs referencing this segment with the new definition.
    let rulesets =
        validate_by_compiling(&state, &project_key, &Change::UpsertSegment(&body)).await?;

    state
        .store
        .upsert_segment(&actor, &project_key, &body)
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

/// `DELETE /projects/{project}/segments/{segment}` -- delete a segment.
///
/// If any environment's flags still reference this segment, the compile
/// will fail with `UnknownSegment` and the deletion will be refused (400).
pub async fn delete_segment<S: Store>(
    State(state): State<AppState<S>>,
    principal: AdminPrincipal,
    Path((project, segment)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal.username;
    let project_key = ProjectKey::new(project).map_err(|e| ApiError::InvalidBody(e.to_string()))?;
    let segment_key = SegmentKey::new(segment).map_err(|e| ApiError::InvalidBody(e.to_string()))?;

    let existing = state
        .store
        .get_segment(&project_key, &segment_key)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    let current_etag = compute_etag(&existing)?;
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    check_if_match(if_match, &current_etag)?;

    // If any env still references this segment, compilation will fail -> 400, deletion refused.
    let rulesets =
        validate_by_compiling(&state, &project_key, &Change::DeleteSegment(&segment_key)).await?;

    state
        .store
        .delete_segment(&actor, &project_key, &segment_key)
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
