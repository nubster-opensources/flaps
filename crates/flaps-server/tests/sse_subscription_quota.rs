//! Integration tests for the `GET /sync/v1/events` concurrency quota
//! (issue #111).
//!
//! Uses axum `oneshot` (no real network socket): the handler runs to
//! completion and returns a `Response` whose body is an unpolled SSE stream.
//! Because the quota permit is acquired synchronously inside the handler
//! (before any stream polling), `oneshot` is sufficient to prove acquisition
//! and rejection without ever driving the SSE body - so these tests never
//! touch the never-ending stream itself and cannot hang on it. Every
//! `oneshot` call is additionally wrapped in a bounded timeout as a
//! belt-and-braces guard against the suite hanging.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use flaps_server::{
    build_router,
    sse_quota::{SseQuota, SseQuotaConfig},
    state::AppState,
};
use flaps_store::{hash::KeyHasher, sqlite::SqliteStore};
use http_body_util::BodyExt;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Upper bound on any single `oneshot` call: opening (not consuming) an SSE
/// stream must resolve near-instantly since the handler never awaits the
/// stream body. A generous bound catches an accidental hang without making
/// the suite slow.
const ONESHOT_BUDGET: Duration = Duration::from_secs(5);

async fn make_store() -> SqliteStore {
    let hasher = KeyHasher::new(b"00000000000000000000000000000000".to_vec());
    SqliteStore::in_memory(hasher)
        .await
        .expect("in-memory store")
}

/// Builds a router whose SSE quota is `max_global` / `max_per_key`, with one
/// project/environment ready to mint server SDK keys.
async fn make_app_with_quota(
    max_global: usize,
    max_per_key: usize,
) -> (axum::Router, AppState<SqliteStore>, String) {
    let store = make_store().await;
    flaps_server::bootstrap_admin(&store, "admin", "admin-pass")
        .await
        .expect("bootstrap");

    let sse_quota = Arc::new(SseQuota::new(SseQuotaConfig {
        max_global,
        max_per_key,
    }));
    let state = AppState::new(store).with_sse_quota(sse_quota);
    let app = build_router(state.clone());

    let token = admin_login(&app).await;
    create_project(&app, "quota-proj", &token).await;
    create_environment(&app, "quota-proj", "quota-env", &token).await;

    (app, state, token)
}

async fn admin_login(app: &axum::Router) -> String {
    let body = serde_json::json!({"username": "admin", "password": "admin-pass"});
    let req = Request::builder()
        .method("POST")
        .uri("/login")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = timeout(app.clone().oneshot(req)).await.unwrap();
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
    let resp = timeout(app.clone().oneshot(req)).await.unwrap();
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
    let resp = timeout(app.clone().oneshot(req)).await.unwrap();
    assert!(
        resp.status().is_success(),
        "create environment: {}",
        resp.status()
    );
}

/// Mints a fresh server SDK key. Each call yields a distinct secret (and
/// therefore a distinct quota key-prefix).
async fn create_server_key(app: &axum::Router, proj: &str, env: &str, token: &str) -> String {
    let body = serde_json::json!({"kind": "server"});
    let req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{proj}/environments/{env}/keys"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = timeout(app.clone().oneshot(req)).await.unwrap();
    assert!(
        resp.status().is_success(),
        "create sdk key: {}",
        resp.status()
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn events_req(sdk_key: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/sync/v1/events")
        .header("Authorization", format!("Bearer {sdk_key}"))
        .body(Body::empty())
        .unwrap()
}

/// Bounds any single oneshot call so an unexpected hang fails the test
/// instead of the suite.
async fn timeout<F: std::future::Future>(fut: F) -> F::Output {
    tokio::time::timeout(ONESHOT_BUDGET, fut)
        .await
        .expect("oneshot call must resolve without hanging")
}

// ---------------------------------------------------------------------------
// Per-key quota
// ---------------------------------------------------------------------------

/// The (max_per_key + 1)-th concurrent subscription for the same key is
/// rejected with 429, while earlier ones stay open.
#[tokio::test]
async fn nth_plus_one_subscription_for_key_is_rejected() {
    let (app, state, token) = make_app_with_quota(10, 2).await;
    let server_key = create_server_key(&app, "quota-proj", "quota-env", &token).await;

    let resp1 = timeout(app.clone().oneshot(events_req(&server_key)))
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK, "first subscription");

    let resp2 = timeout(app.clone().oneshot(events_req(&server_key)))
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK, "second subscription");

    assert_eq!(state.sse_quota.active_subscriptions(), 2);

    let resp3 = timeout(app.clone().oneshot(events_req(&server_key)))
        .await
        .unwrap();
    assert_eq!(
        resp3.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "third subscription for the same key must be rejected"
    );
    assert_eq!(
        state.sse_quota.active_subscriptions(),
        2,
        "the rejected attempt must not count as active"
    );
    assert_eq!(state.sse_quota.rejected_subscriptions(), 1);

    // Release both held responses explicitly so the quota state is clean at
    // the end of the test, keeping every test in this file self-contained.
    drop(resp1);
    drop(resp2);
}

// ---------------------------------------------------------------------------
// Global quota
// ---------------------------------------------------------------------------

/// A generous per-key ceiling does not save a subscription once the global
/// ceiling, shared across different SDK keys, is exhausted.
#[tokio::test]
async fn global_cap_rejects_across_different_keys() {
    let (app, state, token) = make_app_with_quota(2, 10).await;
    let key_a = create_server_key(&app, "quota-proj", "quota-env", &token).await;
    let key_b = create_server_key(&app, "quota-proj", "quota-env", &token).await;
    let key_c = create_server_key(&app, "quota-proj", "quota-env", &token).await;

    let resp_a = timeout(app.clone().oneshot(events_req(&key_a)))
        .await
        .unwrap();
    assert_eq!(resp_a.status(), StatusCode::OK, "key A must succeed");

    let resp_b = timeout(app.clone().oneshot(events_req(&key_b)))
        .await
        .unwrap();
    assert_eq!(resp_b.status(), StatusCode::OK, "key B must succeed");

    let resp_c = timeout(app.clone().oneshot(events_req(&key_c)))
        .await
        .unwrap();
    assert_eq!(
        resp_c.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "key C must be rejected: the global ceiling (2) is already reached"
    );
    assert_eq!(state.sse_quota.active_subscriptions(), 2);
    assert_eq!(state.sse_quota.rejected_subscriptions(), 1);

    drop(resp_a);
    drop(resp_b);
}

// ---------------------------------------------------------------------------
// Permit release
// ---------------------------------------------------------------------------

/// Dropping a held SSE response (standing in for the stream being torn
/// down - client disconnect, cancellation, or shutdown all reduce to this)
/// frees its permit for a subsequent subscription.
#[tokio::test]
async fn dropping_a_subscription_response_frees_its_permit() {
    let (app, state, token) = make_app_with_quota(10, 1).await;
    let server_key = create_server_key(&app, "quota-proj", "quota-env", &token).await;

    let resp1 = timeout(app.clone().oneshot(events_req(&server_key)))
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    assert_eq!(state.sse_quota.active_subscriptions(), 1);

    let rejected = timeout(app.clone().oneshot(events_req(&server_key)))
        .await
        .unwrap();
    assert_eq!(
        rejected.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "per-key ceiling (1) already reached"
    );

    // Drop the held response: releases the permit via Drop, exactly as a
    // real client disconnect, cancellation, or server shutdown would.
    drop(resp1);

    let resp2 = timeout(app.clone().oneshot(events_req(&server_key)))
        .await
        .unwrap();
    assert_eq!(
        resp2.status(),
        StatusCode::OK,
        "slot must be free again after the prior response was dropped"
    );
    assert_eq!(state.sse_quota.active_subscriptions(), 1);

    drop(resp2);
    assert_eq!(state.sse_quota.active_subscriptions(), 0);
}

// ---------------------------------------------------------------------------
// Rejection response shape
// ---------------------------------------------------------------------------

/// The rejection reuses the exact 429 shape as the ordinary rate limiter:
/// status, `Retry-After`, and `application/problem+json` content type.
#[tokio::test]
async fn rejected_subscription_reuses_the_429_problem_json_shape() {
    let (app, _state, token) = make_app_with_quota(0, 0).await;
    let server_key = create_server_key(&app, "quota-proj", "quota-env", &token).await;

    let resp = timeout(app.clone().oneshot(events_req(&server_key)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        resp.headers().get("Retry-After").is_some(),
        "Retry-After must be set"
    );
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert_eq!(content_type, "application/problem+json");

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON body");
    assert_eq!(body["status"].as_u64(), Some(429));
}

// ---------------------------------------------------------------------------
// 403 precedence (client keys never consume the quota)
// ---------------------------------------------------------------------------

/// A client-kind SDK key is rejected with 403 before the quota is ever
/// consulted, so it can never exhaust the quota for other keys.
#[tokio::test]
async fn client_key_is_forbidden_and_never_consumes_the_quota() {
    let (app, state, token) = make_app_with_quota(1, 1).await;
    let body = serde_json::json!({"kind": "client"});
    let req = Request::builder()
        .method("POST")
        .uri("/projects/quota-proj/environments/quota-env/keys")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = timeout(app.clone().oneshot(req)).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let client_key = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_owned();

    let resp = timeout(app.clone().oneshot(events_req(&client_key)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        state.sse_quota.active_subscriptions(),
        0,
        "a forbidden client key must never consume a quota slot"
    );
    assert_eq!(state.sse_quota.rejected_subscriptions(), 0);
}
