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
    etag::{check_if_match, check_if_none_match, compute_etag, read_precondition_header},
    recompile::{Change, evict_project_from_cache, recompile_committed, validate_by_compiling},
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

    // Hold the per-project lock for the whole cycle: precondition check,
    // write and post-commit recompile all happen atomically with respect to
    // any other in-scope mutation for this project (issues #105, #108).
    let lock = state.lock_project(&project_key).await;

    // Scoped so every early exit via `?` below still falls through to the
    // cleanup after: the registry entry must be released on EVERY error
    // exit (a 412 from `check_if_match`, a 422 from a bad precondition
    // header or from `check_if_none_match`, a 422 from `validate_by_compiling`,
    // or a store error), not only when the parent turns out not to exist.
    // Without this, every distinct project key mentioned in a failing PUT
    // would permanently occupy one registry entry.
    let outcome: Result<Response, ApiError> = async {
        // Check If-Match / If-None-Match preconditions (both optional), atomic
        // with the write below since the lock is held across both.
        let existing = state
            .store
            .get_project(&project_key)
            .await
            .map_err(ApiError::from)?;
        let is_create = existing.is_none();

        let current_etag = existing.as_ref().map(compute_etag).transpose()?;
        let if_match = read_precondition_header(&headers, &header::IF_MATCH)?;
        check_if_match(if_match.as_deref(), current_etag.as_deref())?;

        let if_none_match = read_precondition_header(&headers, &header::IF_NONE_MATCH)?;
        check_if_none_match(if_none_match.as_deref(), existing.is_some())?;

        // Compile-as-validation: a clean 422 on invalid input, before any write.
        let rulesets = validate_by_compiling(&state, &project_key, &Change::UpsertProject).await?;
        let affected: Vec<_> = rulesets.into_iter().map(|r| r.environment).collect();

        // Write (its own transaction and audit entry).
        state
            .store
            .upsert_project(&actor, &body)
            .await
            .map_err(ApiError::from)?;

        // Recompile the affected environments from the just-committed store
        // state and install them, replacing any pre-write snapshot (#105).
        recompile_committed(&state, &project_key, &affected).await;

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
                    "This resource is federated; local edits may be overwritten by the \
                     federation.",
                ),
            );
        }
        Ok(response)
    }
    .await;

    // The guard must be dropped BEFORE releasing: `release_project_lock_if_unused`'s
    // `strong_count == 1` gate would otherwise always see this task's own
    // clone and silently no-op, leaking the entry anyway.
    drop(lock);
    if outcome.is_err() {
        // A real, existing project simply gets its entry recreated on the
        // next mutation, so releasing after a transient error here is
        // harmless churn, not a correctness concern; only a caller-chosen
        // key that never resolves to a real project would otherwise leak
        // permanently, which is exactly what this closes.
        state.release_project_lock_if_unused(&project_key);
    }

    outcome
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

    let lock = state.lock_project(&project_key).await;

    // Scoped so every early exit via `?` (including `NotFound`) falls
    // through to the cleanup after: the registry entry must be released on
    // EVERY exit, not only the `NotFound` path. In particular the
    // `If-Match` precondition check deliberately runs BEFORE the existence
    // check (RFC 7232 SS3.1: 412 beats 404 on a missing resource), so a 412
    // here must release too, or a caller probing many never-existing keys
    // with an `If-Match` header would leak one entry per key.
    let outcome: Result<(), ApiError> = async {
        let existing = state
            .store
            .get_project(&project_key)
            .await
            .map_err(ApiError::from)?;

        // Check If-Match before the existence check: a specific-ETag or `*`
        // precondition on a missing resource is a 412, not a 404 (RFC 7232 SS3.1).
        let current_etag = existing.as_ref().map(compute_etag).transpose()?;
        let if_match = read_precondition_header(&headers, &header::IF_MATCH)?;
        check_if_match(if_match.as_deref(), current_etag.as_deref())?;

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

        Ok(())
    }
    .await;

    // The guard must be dropped BEFORE releasing, or the `strong_count == 1`
    // gate always sees this task's own clone and silently no-ops. Release
    // unconditionally: on success the project is gone (no reason to keep the
    // entry); on any error -- `NotFound`, `PreconditionFailed`, or a
    // transient store error on a REAL project -- releasing is harmless
    // churn (the entry is simply recreated on the next request) and closes
    // the leak on every path that previously kept the guard past this point.
    drop(lock);
    state.release_project_lock_if_unused(&project_key);

    outcome.map(|()| StatusCode::NO_CONTENT)
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
