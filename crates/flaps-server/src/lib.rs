//! HTTP server for Flaps.
//!
//! Hosts the admin REST API, the OFREP evaluation endpoints and the ruleset
//! sync channel with server-sent events distribution.

pub mod auth;
pub mod error;
pub mod etag;
pub mod rate_limit;
pub mod recompile;
pub mod routes;
pub mod sse_quota;
pub mod state;
pub mod sync;

use axum::{
    Router,
    routing::{delete, get, post, put},
};

use routes::{
    auth::post_login,
    environment::{delete_environment, get_environment, list_environments, put_environment},
    flag::{delete_flag, get_flag, list_flags, put_flag},
    flag_env_config::{delete_flag_env_config, get_flag_env_config, put_flag_env_config},
    ofrep::{post_evaluate_flag, post_evaluate_flags},
    project::{delete_project, get_project, list_projects, put_project},
    sdk::get_whoami,
    sdk_key::{delete_sdk_key, list_sdk_keys, post_sdk_key},
    segment::{delete_segment, get_segment, list_segments, put_segment},
};
use state::{AppState, Store};
use sync::{get_events, get_ruleset};

/// Builds the full router over the given application state.
///
/// Three families of routes:
///   - Public: no authentication required.
///   - Admin: requires a valid session token (`Authorization: Bearer <token>`).
///   - SDK: requires a valid SDK key (`Authorization: Bearer <key>`), rate-limited.
pub fn build_router<S: Store>(state: AppState<S>) -> Router {
    Router::<AppState<S>>::new()
        // ---- Public ----
        .route("/login", post(post_login::<S>))
        // ---- Admin: CRUD (projects, environments, flags, segments, configs) ----
        .route("/projects", get(list_projects::<S>))
        .route("/projects/{project}", get(get_project::<S>))
        .route("/projects/{project}", put(put_project::<S>))
        .route("/projects/{project}", delete(delete_project::<S>))
        .route(
            "/projects/{project}/environments",
            get(list_environments::<S>),
        )
        .route(
            "/projects/{project}/environments/{env}",
            get(get_environment::<S>),
        )
        .route(
            "/projects/{project}/environments/{env}",
            put(put_environment::<S>),
        )
        .route(
            "/projects/{project}/environments/{env}",
            delete(delete_environment::<S>),
        )
        .route("/projects/{project}/flags", get(list_flags::<S>))
        .route("/projects/{project}/flags/{flag}", get(get_flag::<S>))
        .route("/projects/{project}/flags/{flag}", put(put_flag::<S>))
        .route("/projects/{project}/flags/{flag}", delete(delete_flag::<S>))
        .route("/projects/{project}/segments", get(list_segments::<S>))
        .route(
            "/projects/{project}/segments/{segment}",
            get(get_segment::<S>),
        )
        .route(
            "/projects/{project}/segments/{segment}",
            put(put_segment::<S>),
        )
        .route(
            "/projects/{project}/segments/{segment}",
            delete(delete_segment::<S>),
        )
        .route(
            "/projects/{project}/flags/{flag}/environments/{env}/config",
            get(get_flag_env_config::<S>),
        )
        .route(
            "/projects/{project}/flags/{flag}/environments/{env}/config",
            put(put_flag_env_config::<S>),
        )
        .route(
            "/projects/{project}/flags/{flag}/environments/{env}/config",
            delete(delete_flag_env_config::<S>),
        )
        // ---- Admin: SDK key management ----
        .route(
            "/projects/{project}/environments/{env}/keys",
            post(post_sdk_key::<S>),
        )
        .route(
            "/projects/{project}/environments/{env}/keys",
            get(list_sdk_keys::<S>),
        )
        .route(
            "/projects/{project}/environments/{env}/keys/{prefix}",
            delete(delete_sdk_key::<S>),
        )
        // ---- SDK ----
        .route("/sdk/whoami", get(get_whoami::<S>))
        // ---- OFREP v1 evaluation ----
        .route("/ofrep/v1/evaluate/flags", post(post_evaluate_flags::<S>))
        .route(
            "/ofrep/v1/evaluate/flags/{key}",
            post(post_evaluate_flag::<S>),
        )
        // ---- Sync v1 (server-key only) ----
        .route("/sync/v1/ruleset", get(get_ruleset::<S>))
        .route("/sync/v1/events", get(get_events::<S>))
        .with_state(state)
}

/// Ensures an initial admin account exists.
///
/// Creates the account with `username` and `password` only if the username is not
/// already taken. Returns `Ok(())` in both cases (idempotent bootstrap).
///
/// # Errors
/// Returns a store error if the underlying write fails for a reason other than a
/// username conflict.
pub async fn bootstrap_admin<S: Store>(
    store: &S,
    username: &str,
    password: &str,
) -> Result<(), flaps_store::StoreError> {
    match store.create_account("system", username, password).await {
        Ok(_) | Err(flaps_store::StoreError::Conflict(_)) => Ok(()),
        Err(e) => Err(e),
    }
}
