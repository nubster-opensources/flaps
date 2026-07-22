//! Regression tests for issues #105 (serialize mutations, rebuild the cache
//! from committed store state) and #108 (atomic `If-Match`).
//!
//! Uses axum's `oneshot` (no real network socket) with a `SqliteStore::in_memory`
//! backend, exactly like `tests/admin_api.rs`. Concurrency is driven with
//! `tokio::sync::Barrier` to force genuine overlap between two tasks -- never
//! a timing sleep -- on a multi-thread runtime so the two requests can
//! actually run in parallel.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{HeaderValue, Request, StatusCode, header},
};
use flaps_domain::{
    Environment, EnvironmentKey, Flag, FlagEnvConfig, FlagKey, FlagType, ManagedBy, Project,
    ProjectKey, ServeTarget, ValueType, VariantKey, VariantValue, Variants,
};
use flaps_server::{bootstrap_admin, build_router, state::AppState};
use flaps_store::{hash::KeyHasher, sqlite::SqliteStore};
use http_body_util::BodyExt;
use tokio::sync::Barrier;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test helpers (deliberately duplicated from tests/admin_api.rs: each
// integration test file is its own crate).
// ---------------------------------------------------------------------------

const ADMIN_USER: &str = "test-admin";
const ADMIN_PASS: &str = "test-admin-password";

async fn make_store() -> SqliteStore {
    let hasher = KeyHasher::new(b"00000000000000000000000000000000".to_vec());
    SqliteStore::in_memory(hasher)
        .await
        .expect("in-memory store")
}

/// Builds an app, its `AppState` (for direct cache inspection) and a valid
/// admin session token.
async fn make_authed_app() -> (axum::Router, AppState<SqliteStore>, String) {
    let store = make_store().await;
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .expect("bootstrap admin");
    let state = AppState::new(store);
    let app = build_router(state.clone());

    let login_body = serde_json::json!({
        "username": ADMIN_USER,
        "password": ADMIN_PASS,
    });
    let req = Request::builder()
        .method("POST")
        .uri("/login")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "login must succeed");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let token = json["token"].as_str().unwrap().to_owned();

    (app, state, token)
}

fn project_key(s: &str) -> ProjectKey {
    ProjectKey::new(s).unwrap()
}

fn env_key(s: &str) -> EnvironmentKey {
    EnvironmentKey::new(s).unwrap()
}

fn flag_key(s: &str) -> FlagKey {
    FlagKey::new(s).unwrap()
}

fn variant_key(s: &str) -> VariantKey {
    VariantKey::new(s).unwrap()
}

fn bool_project(key: &str) -> Project {
    Project {
        key: project_key(key),
        name: key.to_owned(),
        description: None,
        external_ref: None,
        managed_by: ManagedBy::Local,
    }
}

fn bool_environment(key: &str) -> Environment {
    Environment {
        key: env_key(key),
        name: key.to_owned(),
        external_ref: None,
        managed_by: ManagedBy::Local,
        metadata: flaps_domain::Metadata::new(),
    }
}

fn bool_flag(key: &str) -> Flag {
    Flag {
        key: flag_key(key),
        name: key.to_owned(),
        description: None,
        flag_type: FlagType::Release,
        value_type: ValueType::Boolean,
        variants: Variants::new(
            ValueType::Boolean,
            [
                (variant_key("on"), VariantValue::Bool(true)),
                (variant_key("off"), VariantValue::Bool(false)),
            ],
        )
        .unwrap(),
        metadata: flaps_domain::Metadata::new(),
    }
}

fn config_with_default(variant: &str) -> FlagEnvConfig {
    FlagEnvConfig {
        enabled: true,
        rules: vec![],
        default_rule: ServeTarget::Fixed(variant_key(variant)),
    }
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

fn extract_etag(response: &axum::response::Response) -> Option<String> {
    response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(std::borrow::ToOwned::to_owned)
}

fn put_req<T: serde::Serialize>(
    uri: &str,
    body: &T,
    token: &str,
    if_match: Option<&str>,
    if_none_match: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"));
    if let Some(etag) = if_match {
        builder = builder.header(
            header::IF_MATCH,
            HeaderValue::from_str(etag).expect("valid header value"),
        );
    }
    if let Some(etag) = if_none_match {
        builder = builder.header(
            header::IF_NONE_MATCH,
            HeaderValue::from_str(etag).expect("valid header value"),
        );
    }
    builder
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn delete_req(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

async fn setup_project_env(app: &axum::Router, token: &str, project: &str, env: &str) {
    let resp = app
        .clone()
        .oneshot(put_req(
            &format!("/projects/{project}"),
            &bool_project(project),
            token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(put_req(
            &format!("/projects/{project}/environments/{env}"),
            &bool_environment(env),
            token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

// ---------------------------------------------------------------------------
// #108: two concurrent updates with the SAME (now-stale-for-one-of-them)
// If-Match ETag must yield exactly one success and one 412.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_put_flag_same_etag_yields_exactly_one_success() {
    let (app, _state, token) = make_authed_app().await;
    setup_project_env(&app, &token, "proj", "prod").await;

    let flag = bool_flag("beta-flag");
    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/beta-flag",
            &flag,
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let etag = extract_etag(&resp).expect("PUT response must carry an ETag");

    let mut updated = flag.clone();
    updated.description = Some("updated concurrently".to_owned());

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let app = app.clone();
        let token = token.clone();
        let etag = etag.clone();
        let updated = updated.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            app.oneshot(put_req(
                "/projects/proj/flags/beta-flag",
                &updated,
                &token,
                Some(&etag),
                None,
            ))
            .await
            .unwrap()
            .status()
        }));
    }

    let mut statuses = Vec::new();
    for handle in handles {
        statuses.push(handle.await.expect("task must not panic"));
    }
    statuses.sort_by_key(StatusCode::as_u16);

    assert_eq!(
        statuses,
        vec![StatusCode::OK, StatusCode::PRECONDITION_FAILED],
        "exactly one concurrent update with the same If-Match ETag must succeed, \
         the other must observe the now-changed ETag and get 412; got {statuses:?}"
    );
}

// ---------------------------------------------------------------------------
// #105: two concurrent mutations to DISTINCT flags in one environment must
// leave BOTH changes in the cached ruleset, with a strictly higher version.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_put_flag_env_config_distinct_flags_both_present_in_cache() {
    let (app, state, token) = make_authed_app().await;
    setup_project_env(&app, &token, "proj", "prod").await;

    for key in ["flag-a", "flag-b"] {
        let resp = app
            .clone()
            .oneshot(put_req(
                &format!("/projects/proj/flags/{key}"),
                &bool_flag(key),
                &token,
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = app
            .clone()
            .oneshot(put_req(
                &format!("/projects/proj/flags/{key}/environments/prod/config"),
                &config_with_default("off"),
                &token,
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    let version_before = {
        let cache = state.cache.read().await;
        cache
            .get(&(project_key("proj"), env_key("prod")))
            .expect("cache must be populated by the setup PUTs")
            .version
    };

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for key in ["flag-a", "flag-b"] {
        let app = app.clone();
        let token = token.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            app.oneshot(put_req(
                &format!("/projects/proj/flags/{key}/environments/prod/config"),
                &config_with_default("on"),
                &token,
                None,
                None,
            ))
            .await
            .unwrap()
            .status()
        }));
    }
    for handle in handles {
        assert_eq!(
            handle.await.expect("task must not panic"),
            StatusCode::OK,
            "each concurrent update targets its own resource; both must succeed"
        );
    }

    let ruleset = {
        let cache = state.cache.read().await;
        cache
            .get(&(project_key("proj"), env_key("prod")))
            .expect("cache entry must still exist after the concurrent updates")
            .clone()
    };

    assert!(
        ruleset.version > version_before,
        "version must be strictly monotone after two content-changing mutations: \
         before={version_before}, after={}",
        ruleset.version
    );

    let doc: serde_json::Value =
        serde_json::from_str(&ruleset.document).expect("compiled document must be valid JSON");
    for key in ["flag-a", "flag-b"] {
        assert_eq!(
            doc["flags"][key]["defaultVariant"].as_str(),
            Some("on"),
            "cached ruleset is missing the concurrent update to {key}: \
             the cache must never be older than a committed, acknowledged mutation. \
             Full document: {doc}"
        );
    }
}

// ---------------------------------------------------------------------------
// #105 (ordering pin): deleting a flag with an existing flag_env_config must
// evict the flag from the environment's cached ruleset, even though deleting
// the flag cascades and deletes the flag_env_config row that a NAIVE
// post-write "affected environments" lookup would rely on.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_flag_with_env_config_recompiles_env_from_committed_state() {
    let (app, state, token) = make_authed_app().await;
    setup_project_env(&app, &token, "proj", "prod").await;

    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/doomed-flag",
            &bool_flag("doomed-flag"),
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/doomed-flag/environments/prod/config",
            &config_with_default("on"),
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    {
        let cache = state.cache.read().await;
        let ruleset = cache
            .get(&(project_key("proj"), env_key("prod")))
            .expect("cache populated after the config PUT");
        let doc: serde_json::Value = serde_json::from_str(&ruleset.document).unwrap();
        assert!(
            doc["flags"].get("doomed-flag").is_some(),
            "sanity check: the flag must be present before deletion"
        );
    }

    let resp = app
        .clone()
        .oneshot(delete_req("/projects/proj/flags/doomed-flag", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let cache = state.cache.read().await;
    let ruleset = cache
        .get(&(project_key("proj"), env_key("prod")))
        .expect("environment must still be compiled (empty flag set), just without the flag");
    let doc: serde_json::Value = serde_json::from_str(&ruleset.document).unwrap();
    assert!(
        doc["flags"].get("doomed-flag").is_none(),
        "the cached ruleset must be recompiled from committed store state after the delete, \
         with the deleted flag gone. Full document: {doc}"
    );
}

// ---------------------------------------------------------------------------
// #108: RFC 7232 semantics at the HTTP layer.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_match_star_on_missing_flag_is_412() {
    let (app, _state, token) = make_authed_app().await;
    setup_project_env(&app, &token, "proj", "prod").await;

    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/ghost-flag",
            &bool_flag("ghost-flag"),
            &token,
            Some("*"),
            None,
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn if_none_match_star_guards_create_only_semantics() {
    let (app, _state, token) = make_authed_app().await;
    setup_project_env(&app, &token, "proj", "prod").await;

    let flag = bool_flag("create-once-flag");

    // First PUT with If-None-Match: * succeeds (resource does not exist yet).
    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/create-once-flag",
            &flag,
            &token,
            None,
            Some("*"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Second PUT with If-None-Match: * fails: the resource now exists.
    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/create-once-flag",
            &flag,
            &token,
            None,
            Some("*"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn if_match_list_matches_any_member() {
    let (app, _state, token) = make_authed_app().await;
    setup_project_env(&app, &token, "proj", "prod").await;

    let flag = bool_flag("list-flag");
    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/list-flag",
            &flag,
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let etag = extract_etag(&resp).unwrap();

    let mut updated = flag.clone();
    updated.description = Some("updated via list If-Match".to_owned());
    let list_header = format!("\"not-the-etag\", \"{etag}\", \"also-not-the-etag\"");

    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/list-flag",
            &updated,
            &token,
            Some(&list_header),
            None,
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(
        body["description"].as_str(),
        Some("updated via list If-Match")
    );
}
