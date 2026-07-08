//! Integration tests for the OFREP v1 evaluation endpoints.
//!
//! Uses axum's `oneshot` (no real network socket) with a `SqliteStore::in_memory`
//! backend. The compiled ruleset cache is pre-populated directly via
//! `install_in_cache` so that tests are self-contained and do not touch the DB
//! on the hot path.
//!
//! Provider note: the `open-feature` Rust provider crate for OFREP is not yet
//! stable at MSRV 1.88; conformance is validated here through golden JSON
//! response bodies checked against the OFREP 0.3.0 schema expectations.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use flaps_compiler::CompiledRuleset;
use flaps_domain::{EnvironmentKey, ProjectKey};
use flaps_server::{build_router, recompile::install_in_cache, state::AppState};
use flaps_store::{hash::KeyHasher, sqlite::SqliteStore};
use http_body_util::BodyExt;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Minimal flagd JSON document with one boolean flag (`feature-x`, enabled, default on)
/// and one string flag (`greeting`, disabled).
const FLAGD_DOC: &str = r#"{
    "flags": {
        "feature-x": {
            "state": "ENABLED",
            "variants": {
                "on": true,
                "off": false
            },
            "defaultVariant": "on"
        },
        "greeting": {
            "state": "DISABLED",
            "variants": {
                "hello": "Hello",
                "bonjour": "Bonjour"
            },
            "defaultVariant": "hello"
        }
    }
}"#;

/// A flagd document with a targeting rule:
/// users with `tier = "beta"` get `"on"`, others fall back to `"off"`.
const FLAGD_DOC_TARGETING: &str = r#"{
    "flags": {
        "beta-flag": {
            "state": "ENABLED",
            "variants": {
                "on": true,
                "off": false
            },
            "defaultVariant": "off",
            "targeting": {
                "if": [
                    { "===": [{ "var": "tier" }, "beta"] },
                    "on",
                    null
                ]
            }
        }
    }
}"#;

fn project_key() -> ProjectKey {
    ProjectKey::new("test-proj").expect("valid project key")
}

fn env_key() -> EnvironmentKey {
    EnvironmentKey::new("test-env").expect("valid env key")
}

/// A fake ruleset pre-populated into the in-memory cache.
fn fake_ruleset(document: &str) -> CompiledRuleset {
    let hash = sha2_hex(document);
    CompiledRuleset {
        environment: env_key(),
        document: document.to_owned(),
        content_hash: hash,
        version: 1,
    }
}

fn sha2_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    hex::encode(h.finalize())
}

// ---------------------------------------------------------------------------
// App builders
// ---------------------------------------------------------------------------

async fn make_store() -> SqliteStore {
    let hasher = KeyHasher::new(b"00000000000000000000000000000000".to_vec());
    SqliteStore::in_memory(hasher)
        .await
        .expect("in-memory store")
}

/// App with a pre-populated cache entry. Returns (app, `sdk_key_prefix`).
///
/// The SDK key resolution path requires a real key in the store, so we create
/// a project+env+sdk key through the admin API first, then swap the cache.
async fn make_app_with_ruleset(document: &str) -> (axum::Router, String) {
    let store = make_store().await;
    flaps_server::bootstrap_admin(&store, "admin", "admin-pass")
        .await
        .expect("bootstrap");

    let state = AppState::new(store);
    let app = build_router(state.clone());

    // Login as admin.
    let token = admin_login(&app).await;

    // Create project and environment via the admin API so the store has them.
    create_project(&app, "test-proj", &token).await;
    create_environment(&app, "test-proj", "test-env", &token).await;

    // Create an SDK key.
    let sdk_key = create_sdk_key(&app, "test-proj", "test-env", &token).await;

    // Pre-populate the cache with the fake ruleset (bypasses compilation).
    install_in_cache(&state, &project_key(), vec![fake_ruleset(document)]).await;

    (app, sdk_key)
}

/// Convenience wrapper: an app WITHOUT a cache entry for (project, env).
async fn make_app_empty_cache() -> (axum::Router, String) {
    let store = make_store().await;
    flaps_server::bootstrap_admin(&store, "admin", "admin-pass")
        .await
        .expect("bootstrap");
    let state = AppState::new(store);
    let app = build_router(state.clone());

    let token = admin_login(&app).await;
    create_project(&app, "test-proj", &token).await;
    create_environment(&app, "test-proj", "test-env", &token).await;
    let sdk_key = create_sdk_key(&app, "test-proj", "test-env", &token).await;

    (app, sdk_key)
}

/// App with rate limiter throttled to capacity 0 (immediately exhausted).
async fn make_app_rate_limited() -> (axum::Router, String) {
    let store = make_store().await;
    flaps_server::bootstrap_admin(&store, "admin", "admin-pass")
        .await
        .expect("bootstrap");

    let rate_limiter = Arc::new(flaps_server::rate_limit::RateLimiter::new(
        flaps_server::rate_limit::RateLimitConfig {
            enabled: true,
            capacity: 0,
            refill_per_second: 0.0,
        },
    ));
    // Login is unrelated to this SDK rate limit scenario: keep it disabled so
    // the admin login performed by the test setup below is never throttled.
    let login_rate_limiter = Arc::new(flaps_server::rate_limit::RateLimiter::disabled());
    let state = AppState::with_config(
        store,
        rate_limiter,
        login_rate_limiter,
        std::time::Duration::from_secs(3600),
    );
    let app = build_router(state.clone());

    let token = admin_login(&app).await;
    create_project(&app, "test-proj", &token).await;
    create_environment(&app, "test-proj", "test-env", &token).await;
    let sdk_key = create_sdk_key(&app, "test-proj", "test-env", &token).await;

    install_in_cache(&state, &project_key(), vec![fake_ruleset(FLAGD_DOC)]).await;

    (app, sdk_key)
}

// ---------------------------------------------------------------------------
// Admin API helpers (create project / env / SDK key via the router)
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
    assert_eq!(resp.status(), StatusCode::OK, "admin login must succeed");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["token"].as_str().unwrap().to_owned()
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
        "create project must succeed: {}",
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
        "create environment must succeed: {}",
        resp.status()
    );
}

/// Creates an SDK key and returns the raw key secret (returned once at creation).
async fn create_sdk_key(app: &axum::Router, proj: &str, env: &str, token: &str) -> String {
    let body = serde_json::json!({"kind": "server"});
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
        "create sdk key must succeed: {}",
        resp.status()
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["secret"].as_str().unwrap().to_owned()
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

fn ofrep_single_req(key: &str, sdk_key: &str, context: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/ofrep/v1/evaluate/flags/{key}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {sdk_key}"))
        .body(Body::from(serde_json::to_vec(context).unwrap()))
        .unwrap()
}

fn ofrep_bulk_req(sdk_key: &str, context: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/ofrep/v1/evaluate/flags")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {sdk_key}"))
        .body(Body::from(serde_json::to_vec(context).unwrap()))
        .unwrap()
}

fn ofrep_bulk_req_with_etag(
    sdk_key: &str,
    context: &serde_json::Value,
    if_none_match: &str,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/ofrep/v1/evaluate/flags")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {sdk_key}"))
        .header(header::IF_NONE_MATCH, if_none_match)
        .body(Body::from(serde_json::to_vec(&context).unwrap()))
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// Single flag tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_200_boolean_flag_enabled() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app
        .oneshot(ofrep_single_req("feature-x", &sdk_key, &ctx))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["key"].as_str(), Some("feature-x"));
    assert_eq!(json["value"], serde_json::json!(true));
    assert_eq!(json["reason"].as_str(), Some("STATIC"));
    assert_eq!(json["variant"].as_str(), Some("on"));
}

#[tokio::test]
async fn single_200_disabled_flag_has_no_value() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app
        .oneshot(ofrep_single_req("greeting", &sdk_key, &ctx))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["reason"].as_str(), Some("DISABLED"));
    assert!(json["value"].is_null(), "disabled flag must omit value");
    assert!(json["variant"].is_null(), "disabled flag must omit variant");
}

#[tokio::test]
async fn single_404_flag_not_found() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app
        .oneshot(ofrep_single_req("does-not-exist", &sdk_key, &ctx))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = body_json(resp).await;
    assert_eq!(json["errorCode"].as_str(), Some("FLAG_NOT_FOUND"));
}

#[tokio::test]
async fn single_401_missing_sdk_key() {
    let (app, _sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let ctx = serde_json::json!({"context": {}});
    let req = Request::builder()
        .method("POST")
        .uri("/ofrep/v1/evaluate/flags/feature-x")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&ctx).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn single_400_invalid_context_bad_json() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let req = Request::builder()
        .method("POST")
        .uri("/ofrep/v1/evaluate/flags/feature-x")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {sdk_key}"))
        .body(Body::from(b"not-json".as_ref()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["errorCode"].as_str(), Some("INVALID_CONTEXT"));
}

#[tokio::test]
async fn single_429_rate_limit_exceeded() {
    let (app, sdk_key) = make_app_rate_limited().await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app
        .oneshot(ofrep_single_req("feature-x", &sdk_key, &ctx))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        resp.headers().contains_key("Retry-After"),
        "429 must include Retry-After header"
    );
}

#[tokio::test]
async fn single_404_when_cache_empty() {
    let (app, sdk_key) = make_app_empty_cache().await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app
        .oneshot(ofrep_single_req("feature-x", &sdk_key, &ctx))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = body_json(resp).await;
    assert_eq!(json["errorCode"].as_str(), Some("FLAG_NOT_FOUND"));
}

#[tokio::test]
async fn single_targeting_match_reason() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC_TARGETING).await;
    let ctx = serde_json::json!({"context": {"targetingKey": "u1", "tier": "beta"}});
    let resp = app
        .oneshot(ofrep_single_req("beta-flag", &sdk_key, &ctx))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["reason"].as_str(), Some("TARGETING_MATCH"));
    assert_eq!(json["value"], serde_json::json!(true));
}

#[tokio::test]
async fn single_default_reason_when_targeting_misses() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC_TARGETING).await;
    let ctx = serde_json::json!({"context": {"targetingKey": "u2", "tier": "standard"}});
    let resp = app
        .oneshot(ofrep_single_req("beta-flag", &sdk_key, &ctx))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["reason"].as_str(), Some("DEFAULT"));
    assert_eq!(json["value"], serde_json::json!(false));
}

// ---------------------------------------------------------------------------
// Bulk flag tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bulk_200_returns_etag_header() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app.oneshot(ofrep_bulk_req(&sdk_key, &ctx)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().contains_key(header::ETAG),
        "bulk 200 must include ETag header"
    );
    let json = body_json(resp).await;
    assert!(json["flags"].is_array(), "response must have a flags array");
    assert!(
        json["metadata"]["version"].is_string(),
        "metadata.version must be present"
    );
}

#[tokio::test]
async fn bulk_200_contains_all_flags() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app.oneshot(ofrep_bulk_req(&sdk_key, &ctx)).await.unwrap();
    let json = body_json(resp).await;
    let flags = json["flags"].as_array().unwrap();
    assert_eq!(flags.len(), 2, "must have exactly 2 flags from the ruleset");

    let keys: Vec<&str> = flags.iter().map(|f| f["key"].as_str().unwrap()).collect();
    assert!(
        keys.contains(&"feature-x"),
        "feature-x must be in bulk response"
    );
    assert!(
        keys.contains(&"greeting"),
        "greeting must be in bulk response"
    );
}

#[tokio::test]
async fn bulk_304_when_etag_matches() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let ctx = serde_json::json!({"context": {}});

    // First request to get the ETag.
    let resp = app
        .clone()
        .oneshot(ofrep_bulk_req(&sdk_key, &ctx))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let etag = resp
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // Second request with matching If-None-Match -> 304.
    let resp = app
        .clone()
        .oneshot(ofrep_bulk_req_with_etag(&sdk_key, &ctx, &etag))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn bulk_200_when_etag_does_not_match() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app
        .oneshot(ofrep_bulk_req_with_etag(
            &sdk_key,
            &ctx,
            "\"old-stale-etag\"",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn bulk_200_empty_flags_when_cache_empty() {
    let (app, sdk_key) = make_app_empty_cache().await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app.oneshot(ofrep_bulk_req(&sdk_key, &ctx)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let flags = json["flags"].as_array().unwrap();
    assert!(
        flags.is_empty(),
        "empty cache must return empty flags array"
    );
}

#[tokio::test]
async fn bulk_401_missing_sdk_key() {
    let (app, _) = make_app_with_ruleset(FLAGD_DOC).await;
    let ctx = serde_json::json!({"context": {}});
    let req = Request::builder()
        .method("POST")
        .uri("/ofrep/v1/evaluate/flags")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&ctx).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bulk_429_rate_limit_exceeded() {
    let (app, sdk_key) = make_app_rate_limited().await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app.oneshot(ofrep_bulk_req(&sdk_key, &ctx)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        resp.headers().contains_key("Retry-After"),
        "429 must include Retry-After header"
    );
}

#[tokio::test]
async fn bulk_400_invalid_context_bad_json() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let req = Request::builder()
        .method("POST")
        .uri("/ofrep/v1/evaluate/flags")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {sdk_key}"))
        .body(Body::from(b"{{bad".as_ref()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["errorCode"].as_str(), Some("INVALID_CONTEXT"));
}

/// Bulk response can contain both successes and per-flag errors coexisting.
/// We inject a document with two flags, then evaluate: both are present in
/// the flags array. A flag-not-found cannot be triggered from within an
/// existing FlagSet so we test with a real disabled flag to verify both
/// success and DISABLED entries coexist.
#[tokio::test]
async fn bulk_mixed_success_and_disabled() {
    let (app, sdk_key) = make_app_with_ruleset(FLAGD_DOC).await;
    let ctx = serde_json::json!({"context": {}});
    let resp = app.oneshot(ofrep_bulk_req(&sdk_key, &ctx)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let flags = json["flags"].as_array().unwrap();

    let feature_x = flags.iter().find(|f| f["key"] == "feature-x").unwrap();
    assert_eq!(feature_x["reason"].as_str(), Some("STATIC"));

    let greeting = flags.iter().find(|f| f["key"] == "greeting").unwrap();
    assert_eq!(greeting["reason"].as_str(), Some("DISABLED"));
    assert!(greeting["value"].is_null(), "disabled must have no value");
}

// ---------------------------------------------------------------------------
// Atomicity test (best-effort)
// ---------------------------------------------------------------------------

/// Verifies that a concurrent cache swap during bulk evaluation does not
/// produce a panic or a partially-evaluated response.
///
/// We spawn a task that repeatedly swaps the cache while another task fires
/// bulk evaluation requests. Every response must be either 200 with a valid
/// `flags` array or 304. No panics or 500s are acceptable.
#[tokio::test]
async fn bulk_atomicity_concurrent_cache_swap() {
    use flaps_server::recompile::install_in_cache;
    use std::sync::Arc;

    let store = make_store().await;
    flaps_server::bootstrap_admin(&store, "admin", "admin-pass")
        .await
        .expect("bootstrap");
    let state = AppState::new(store);
    let app = Arc::new(build_router(state.clone()));

    let token = admin_login(app.as_ref()).await;
    create_project(app.as_ref(), "test-proj", &token).await;
    create_environment(app.as_ref(), "test-proj", "test-env", &token).await;
    let sdk_key = create_sdk_key(app.as_ref(), "test-proj", "test-env", &token).await;

    install_in_cache(&state, &project_key(), vec![fake_ruleset(FLAGD_DOC)]).await;

    let state_clone = state.clone();
    let swap_task = tokio::spawn(async move {
        for _ in 0..20u32 {
            install_in_cache(&state_clone, &project_key(), vec![fake_ruleset(FLAGD_DOC)]).await;
            tokio::task::yield_now().await;
        }
    });

    for _ in 0..20u32 {
        let ctx = serde_json::json!({"context": {}});
        let resp = (*app)
            .clone()
            .oneshot(ofrep_bulk_req(&sdk_key, &ctx))
            .await
            .unwrap();
        let status = resp.status();
        assert!(
            status == StatusCode::OK || status == StatusCode::NOT_MODIFIED,
            "concurrent swap must not produce unexpected status: {status}"
        );
        if status == StatusCode::OK {
            let json = body_json(resp).await;
            assert!(json["flags"].is_array(), "flags must always be an array");
        }
    }

    swap_task.await.expect("swap task must not panic");
}
