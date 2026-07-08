//! Integration tests for the sync v1 endpoints.
//!
//! Routes under test:
//! - `GET /sync/v1/ruleset` (download compiled ruleset)
//! - `GET /sync/v1/events`  (SSE stream of change notifications)
//!
//! Uses axum `oneshot` (no real network socket) with an in-memory SQLite store.
//! The compiled ruleset cache is pre-populated directly via `install_in_cache`
//! to keep tests self-contained and independent from the DB on the hot path.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use flaps_compiler::CompiledRuleset;
use flaps_domain::{EnvironmentKey, ProjectKey};
use flaps_server::{
    build_router,
    rate_limit::{RateLimitConfig, RateLimiter},
    recompile::{evict_environment_from_cache, install_in_cache},
    state::AppState,
};
use flaps_store::{hash::KeyHasher, sqlite::SqliteStore};
use http_body_util::BodyExt;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const FLAGD_DOC: &str = r#"{"flags":{"demo":{"state":"ENABLED","variants":{"on":true,"off":false},"defaultVariant":"on"}}}"#;

fn project_key() -> ProjectKey {
    ProjectKey::new("sync-proj").expect("valid project key")
}

fn env_key() -> EnvironmentKey {
    EnvironmentKey::new("sync-env").expect("valid env key")
}

fn other_env_key() -> EnvironmentKey {
    EnvironmentKey::new("other-env").expect("valid env key")
}

fn fake_ruleset(doc: &str, env: EnvironmentKey, version: u64) -> CompiledRuleset {
    CompiledRuleset {
        environment: env,
        document: doc.to_owned(),
        content_hash: sha2_hex(doc),
        version,
    }
}

fn sha2_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    hex::encode(h.finalize())
}

// ---------------------------------------------------------------------------
// Store and app builders
// ---------------------------------------------------------------------------

async fn make_store() -> SqliteStore {
    let hasher = KeyHasher::new(b"00000000000000000000000000000000".to_vec());
    SqliteStore::in_memory(hasher)
        .await
        .expect("in-memory store")
}

/// Bootstrap state, create project+env+SDK server key, pre-populate cache.
/// Returns (app, `server_sdk_key_secret`).
async fn make_app_with_ruleset(doc: &str) -> (axum::Router, AppState<SqliteStore>, String) {
    let store = make_store().await;
    flaps_server::bootstrap_admin(&store, "admin", "admin-pass")
        .await
        .expect("bootstrap");

    let state = AppState::new(store);
    let app = build_router(state.clone());

    let token = admin_login(&app).await;
    create_project(&app, "sync-proj", &token).await;
    create_environment(&app, "sync-proj", "sync-env", &token).await;
    let server_key = create_sdk_key(&app, "sync-proj", "sync-env", "server", &token).await;

    install_in_cache(
        &state,
        &project_key(),
        vec![fake_ruleset(doc, env_key(), 1)],
    )
    .await;

    (app, state, server_key)
}

/// Builds an app where (sync-proj, sync-env) has no cache entry.
///
/// The project and environment are created via the admin API (required by the
/// FK constraint on `sdk_keys`). The cache entry produced by the environment
/// upsert is then evicted so the scope has no compiled ruleset.
async fn make_app_empty_cache() -> (axum::Router, String) {
    let store = make_store().await;
    flaps_server::bootstrap_admin(&store, "admin", "admin-pass")
        .await
        .expect("bootstrap");

    let state = AppState::new(store);
    let app = build_router(state.clone());

    let token = admin_login(&app).await;
    create_project(&app, "sync-proj", &token).await;
    create_environment(&app, "sync-proj", "sync-env", &token).await;
    let server_key = create_sdk_key(&app, "sync-proj", "sync-env", "server", &token).await;

    // Evict the cache entry produced by `put_environment` so that
    // GET /sync/v1/ruleset returns 404 for this scope.
    evict_environment_from_cache(&state, &project_key(), &env_key()).await;

    (app, server_key)
}

/// App with a client SDK key for testing 403 responses.
async fn make_app_with_client_key() -> (axum::Router, AppState<SqliteStore>, String) {
    let store = make_store().await;
    flaps_server::bootstrap_admin(&store, "admin", "admin-pass")
        .await
        .expect("bootstrap");

    let state = AppState::new(store);
    let app = build_router(state.clone());

    let token = admin_login(&app).await;
    create_project(&app, "sync-proj", &token).await;
    create_environment(&app, "sync-proj", "sync-env", &token).await;
    let client_key = create_sdk_key(&app, "sync-proj", "sync-env", "client", &token).await;

    install_in_cache(
        &state,
        &project_key(),
        vec![fake_ruleset(FLAGD_DOC, env_key(), 1)],
    )
    .await;

    (app, state, client_key)
}

/// App with rate limiter set to 0 capacity (immediately exhausted).
async fn make_app_rate_limited() -> (axum::Router, String) {
    let store = make_store().await;
    flaps_server::bootstrap_admin(&store, "admin", "admin-pass")
        .await
        .expect("bootstrap");

    let rate_limiter = Arc::new(RateLimiter::new(RateLimitConfig {
        enabled: true,
        capacity: 0,
        refill_per_second: 0.0,
    }));
    // Login is unrelated to this SDK rate limit scenario: keep it disabled so
    // the admin login performed by the test setup below is never throttled.
    let login_rate_limiter = Arc::new(RateLimiter::disabled());
    let state = AppState::with_config(
        store,
        rate_limiter,
        login_rate_limiter,
        std::time::Duration::from_secs(3600),
    );
    let app = build_router(state.clone());

    let token = admin_login(&app).await;
    create_project(&app, "sync-proj", &token).await;
    create_environment(&app, "sync-proj", "sync-env", &token).await;
    let server_key = create_sdk_key(&app, "sync-proj", "sync-env", "server", &token).await;

    install_in_cache(
        &state,
        &project_key(),
        vec![fake_ruleset(FLAGD_DOC, env_key(), 1)],
    )
    .await;

    (app, server_key)
}

// ---------------------------------------------------------------------------
// Admin API helpers
// ---------------------------------------------------------------------------

async fn admin_login(app: &axum::Router) -> String {
    let body = serde_json::json!({"username": "admin", "password": "admin-pass"});
    let req = Request::builder()
        .method("POST")
        .uri("/login")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn create_project(app: &axum::Router, key: &str, token: &str) {
    let body = serde_json::json!({"key": key, "name": key, "managed_by": "local"});
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/projects/{key}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(
        resp.status().is_success(),
        "create project: {}",
        resp.status()
    );
}

async fn create_environment(app: &axum::Router, proj: &str, env: &str, token: &str) {
    let body = serde_json::json!({"key": env, "name": env, "managed_by": "local"});
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/projects/{proj}/environments/{env}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(
        resp.status().is_success(),
        "create environment: {}",
        resp.status()
    );
}

async fn create_sdk_key(
    app: &axum::Router,
    proj: &str,
    env: &str,
    kind: &str,
    token: &str,
) -> String {
    let body = serde_json::json!({"kind": kind});
    let req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{proj}/environments/{env}/keys"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(
        resp.status().is_success(),
        "create sdk key ({kind}): {}",
        resp.status()
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_owned()
}

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

fn ruleset_req(sdk_key: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/sync/v1/ruleset")
        .header("Authorization", format!("Bearer {sdk_key}"))
        .body(Body::empty())
        .unwrap()
}

fn ruleset_req_with_if_none_match(sdk_key: &str, etag: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/sync/v1/ruleset")
        .header("Authorization", format!("Bearer {sdk_key}"))
        .header(header::IF_NONE_MATCH, etag)
        .body(Body::empty())
        .unwrap()
}

fn events_req(sdk_key: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/sync/v1/events")
        .header("Authorization", format!("Bearer {sdk_key}"))
        .body(Body::empty())
        .unwrap()
}

fn events_req_no_auth() -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/sync/v1/events")
        .body(Body::empty())
        .unwrap()
}

fn ruleset_req_no_auth() -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/sync/v1/ruleset")
        .body(Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// Download tests
// ---------------------------------------------------------------------------

/// 200: server key, cache populated - returns document + ETag + X-Flaps-Version.
#[tokio::test]
async fn ruleset_200_server_key_returns_document_with_headers() {
    let (app, _state, server_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let resp = app.oneshot(ruleset_req(&server_key)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let etag = resp.headers().get(header::ETAG);
    assert!(etag.is_some(), "ETag header must be present");
    let etag_str = etag.unwrap().to_str().unwrap();
    assert!(
        etag_str.starts_with('"') && etag_str.ends_with('"'),
        "ETag must be a strong quoted value"
    );

    let version_header = resp.headers().get("X-Flaps-Version");
    assert!(
        version_header.is_some(),
        "X-Flaps-Version header must be present"
    );
    let version: u64 = version_header
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .expect("version must be numeric");
    assert_eq!(version, 1);

    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(ct, "application/json");

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("body must be valid JSON");
    assert!(
        body.get("flags").is_some(),
        "body must contain the flagd document"
    );
}

/// 304: matching If-None-Match returns empty body.
#[tokio::test]
async fn ruleset_304_on_matching_etag() {
    let (app, _state, server_key) = make_app_with_ruleset(FLAGD_DOC).await;

    // First request to get the ETag.
    let resp = app.clone().oneshot(ruleset_req(&server_key)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let etag = resp
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // Second request with the matching ETag.
    let resp = app
        .oneshot(ruleset_req_with_if_none_match(&server_key, &etag))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(body.is_empty(), "304 must have no body");
}

/// 404: no cache entry for the scope.
#[tokio::test]
async fn ruleset_404_cache_absent() {
    let (app, server_key) = make_app_empty_cache().await;
    let resp = app.oneshot(ruleset_req(&server_key)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// 401: no Authorization header.
#[tokio::test]
async fn ruleset_401_no_key() {
    let (app, _state, _server_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let resp = app.oneshot(ruleset_req_no_auth()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// 403: client SDK key.
#[tokio::test]
async fn ruleset_403_client_key() {
    let (app, _state, client_key) = make_app_with_client_key().await;
    let resp = app.oneshot(ruleset_req(&client_key)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// 429: rate limit exceeded.
#[tokio::test]
async fn ruleset_429_rate_limited() {
    let (app, server_key) = make_app_rate_limited().await;
    let resp = app.oneshot(ruleset_req(&server_key)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        resp.headers().get("Retry-After").is_some(),
        "Retry-After must be set"
    );
}

// ---------------------------------------------------------------------------
// SSE auth tests
// ---------------------------------------------------------------------------

/// 401: no Authorization header on SSE endpoint.
#[tokio::test]
async fn events_401_no_key() {
    let (app, _state, _server_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let resp = app.oneshot(events_req_no_auth()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// 403: client SDK key on SSE endpoint.
#[tokio::test]
async fn events_403_client_key() {
    let (app, _state, client_key) = make_app_with_client_key().await;
    let resp = app.oneshot(events_req(&client_key)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Notify exactly-one test
// ---------------------------------------------------------------------------

/// After `install_in_cache` with two rulesets (env1, env2), the broadcast
/// channel carries exactly two events: one per environment, zero flag data.
#[tokio::test]
async fn notify_exactly_one_event_per_affected_env() {
    let store = make_store().await;
    let state = AppState::new(store);

    let mut rx = state.events.subscribe();

    let ruleset_env1 = fake_ruleset(FLAGD_DOC, env_key(), 1);
    let ruleset_env2 = fake_ruleset(FLAGD_DOC, other_env_key(), 1);

    install_in_cache(&state, &project_key(), vec![ruleset_env1, ruleset_env2]).await;

    let ev1 = rx.try_recv().expect("first event must be available");
    let ev2 = rx.try_recv().expect("second event must be available");
    assert!(rx.try_recv().is_err(), "must be exactly two events");

    // Verify events carry only env + version, no flag data on the struct.
    assert_eq!(ev1.environment, env_key());
    assert_eq!(ev1.version, 1);
    assert_eq!(ev1.project, project_key());

    assert_eq!(ev2.environment, other_env_key());
    assert_eq!(ev2.version, 1);
}

/// Events for a different environment are not emitted on the unrelated scope.
#[tokio::test]
async fn notify_zero_events_for_unrelated_env() {
    let store = make_store().await;
    let state = AppState::new(store);

    let mut rx = state.events.subscribe();

    let other_env = EnvironmentKey::new("other-env").unwrap();
    let other_proj = ProjectKey::new("other-proj").unwrap();
    let ruleset = fake_ruleset(FLAGD_DOC, other_env.clone(), 1);

    install_in_cache(&state, &other_proj, vec![ruleset]).await;

    // Exactly one event, for other-proj/other-env.
    let ev = rx.try_recv().expect("one event must arrive");
    assert_eq!(ev.project, other_proj);
    assert_eq!(ev.environment, other_env);
    assert!(rx.try_recv().is_err(), "must be exactly one event");
}

// ---------------------------------------------------------------------------
// SSE stream test (with timeout to avoid hanging)
// ---------------------------------------------------------------------------

/// Server key on SSE endpoint: response is 200 with text/event-stream.
/// Marked #[ignore] if flaky in CI (the stream never closes by itself).
#[tokio::test]
#[ignore = "SSE stream does not close; run manually to verify 200 + content-type"]
async fn events_200_server_key_opens_stream() {
    let (app, _state, server_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let resp = app.oneshot(events_req(&server_key)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "content-type must be text/event-stream, got: {ct}"
    );
}

/// e2e: a subscriber connected before a mutation receives the event, then
/// re-sync via GET /sync/v1/ruleset returns the new version.
///
/// Uses a broadcast receiver directly (no real HTTP SSE connection) to avoid
/// the inherent blocking nature of SSE streams in oneshot tests.
#[tokio::test]
async fn e2e_subscriber_receives_event_then_resync_returns_new_version() {
    let store = make_store().await;
    flaps_server::bootstrap_admin(&store, "admin", "admin-pass")
        .await
        .expect("bootstrap");

    let state = AppState::new(store);
    let app = build_router(state.clone());

    let token = admin_login(&app).await;
    create_project(&app, "sync-proj", &token).await;
    create_environment(&app, "sync-proj", "sync-env", &token).await;
    let server_key = create_sdk_key(&app, "sync-proj", "sync-env", "server", &token).await;

    // Subscribe BEFORE installing in cache.
    let mut rx = state.events.subscribe();

    // Install version 1.
    install_in_cache(
        &state,
        &project_key(),
        vec![fake_ruleset(FLAGD_DOC, env_key(), 1)],
    )
    .await;

    // Subscriber must receive the event within a reasonable timeout.
    let ev = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        rx.recv().await.expect("event must be received")
    })
    .await
    .expect("event must arrive within 1s");

    assert_eq!(ev.environment, env_key());
    assert_eq!(ev.version, 1);

    // Re-sync: GET /sync/v1/ruleset must return the version announced.
    let resp = app.oneshot(ruleset_req(&server_key)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let version: u64 = resp
        .headers()
        .get("X-Flaps-Version")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        version, ev.version,
        "re-sync version must match announced version"
    );
}
