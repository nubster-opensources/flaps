//! HTTP server for Flaps.
//!
//! Hosts the admin REST API, the OFREP evaluation endpoints and the ruleset
//! sync channel with server-sent events distribution. The server lands with
//! the v0.1.0 milestone.

pub mod actor;
pub mod error;
pub mod etag;
pub mod recompile;
pub mod routes;
pub mod state;

use axum::{
    Router,
    routing::{delete, get, put},
};

use routes::{
    environment::{delete_environment, get_environment, list_environments, put_environment},
    flag::{delete_flag, get_flag, list_flags, put_flag},
    flag_env_config::{delete_flag_env_config, get_flag_env_config, put_flag_env_config},
    project::{delete_project, get_project, list_projects, put_project},
    segment::{delete_segment, get_segment, list_segments, put_segment},
};
use state::{AppState, Store};

/// Builds the admin router over the given application state.
pub fn build_router<S: Store>(state: AppState<S>) -> Router {
    Router::<AppState<S>>::new()
        // Projects
        .route("/projects", get(list_projects::<S>))
        .route("/projects/:project", get(get_project::<S>))
        .route("/projects/:project", put(put_project::<S>))
        .route("/projects/:project", delete(delete_project::<S>))
        // Environments
        .route(
            "/projects/:project/environments",
            get(list_environments::<S>),
        )
        .route(
            "/projects/:project/environments/:env",
            get(get_environment::<S>),
        )
        .route(
            "/projects/:project/environments/:env",
            put(put_environment::<S>),
        )
        .route(
            "/projects/:project/environments/:env",
            delete(delete_environment::<S>),
        )
        // Flags
        .route("/projects/:project/flags", get(list_flags::<S>))
        .route("/projects/:project/flags/:flag", get(get_flag::<S>))
        .route("/projects/:project/flags/:flag", put(put_flag::<S>))
        .route("/projects/:project/flags/:flag", delete(delete_flag::<S>))
        // Segments
        .route("/projects/:project/segments", get(list_segments::<S>))
        .route(
            "/projects/:project/segments/:segment",
            get(get_segment::<S>),
        )
        .route(
            "/projects/:project/segments/:segment",
            put(put_segment::<S>),
        )
        .route(
            "/projects/:project/segments/:segment",
            delete(delete_segment::<S>),
        )
        // FlagEnvConfig
        .route(
            "/projects/:project/flags/:flag/environments/:env/config",
            get(get_flag_env_config::<S>),
        )
        .route(
            "/projects/:project/flags/:flag/environments/:env/config",
            put(put_flag_env_config::<S>),
        )
        .route(
            "/projects/:project/flags/:flag/environments/:env/config",
            delete(delete_flag_env_config::<S>),
        )
        .with_state(state)
}
