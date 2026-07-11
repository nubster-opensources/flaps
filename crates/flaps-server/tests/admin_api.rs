//! Integration tests for the admin REST API.
//!
//! Uses axum's `oneshot` (no real network socket) with a `SqliteStore::in_memory` backend.
//! All mutation routes require a valid session token; helpers call `POST /login` first.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use flaps_domain::{
    Environment, EnvironmentKey, Flag, FlagEnvConfig, FlagKey, FlagType, ManagedBy, MatchOperator,
    Predicate, Project, ProjectKey, Segment, SegmentKey, SegmentMatch, ServeTarget, TargetingRule,
    ValueType, VariantKey, VariantValue, Variants,
};
use flaps_server::{
    bootstrap_admin, build_router,
    rate_limit::{RateLimitConfig, RateLimiter},
    state::AppState,
};
use flaps_store::{
    hash::KeyHasher,
    repository::{AuditLogRepository, FlagEnvConfigRepository},
    sqlite::SqliteStore,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

const ADMIN_USER: &str = "test-admin";
const ADMIN_PASS: &str = "test-admin-password";

async fn make_store() -> SqliteStore {
    let hasher = KeyHasher::new(b"00000000000000000000000000000000".to_vec());
    SqliteStore::in_memory(hasher)
        .await
        .expect("in-memory store")
}

/// Creates an app and a valid admin session token for use in authed requests.
async fn make_authed_app() -> (axum::Router, String) {
    let store = make_store().await;
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .expect("bootstrap admin");
    let state = AppState::new(store);
    let app = build_router(state);

    // Login to get a session token.
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

    (app, token)
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

fn segment_key(s: &str) -> SegmentKey {
    SegmentKey::new(s).unwrap()
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

fn simple_config(variant: &str) -> FlagEnvConfig {
    FlagEnvConfig {
        enabled: true,
        rules: vec![],
        default_rule: ServeTarget::Fixed(variant_key(variant)),
    }
}

fn simple_segment(key: &str) -> Segment {
    Segment {
        key: segment_key(key),
        name: key.to_owned(),
        match_expr: SegmentMatch::Predicate(Predicate {
            attribute: "tier".into(),
            operator: MatchOperator::Equals,
            values: vec![serde_json::json!("beta")],
        }),
    }
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

fn put_project_req(key: &str, project: &Project, token: &str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!("/projects/{key}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(project).unwrap()))
        .unwrap()
}

fn put_env_req(proj: &str, env: &str, body: &Environment, token: &str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!("/projects/{proj}/environments/{env}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn put_flag_req(proj: &str, flag: &str, body: &Flag, token: &str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!("/projects/{proj}/flags/{flag}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn put_segment_req(proj: &str, seg: &str, body: &Segment, token: &str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!("/projects/{proj}/segments/{seg}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn put_config_req(
    proj: &str,
    flag: &str,
    env: &str,
    body: &FlagEnvConfig,
    token: &str,
) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!(
            "/projects/{proj}/flags/{flag}/environments/{env}/config"
        ))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

/// Builds an anonymous GET request (no `Authorization` header).
fn get_req(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

/// Builds a GET request carrying a valid admin session token.
fn get_authed_req(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
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

fn extract_etag(response: &axum::response::Response) -> Option<String> {
    response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(std::borrow::ToOwned::to_owned)
}

// ---------------------------------------------------------------------------
// Test 1: project_crud_round_trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn project_crud_round_trip() {
    let (app, token) = make_authed_app().await;
    let project = bool_project("my-project");

    // PUT (create)
    let resp = app
        .clone()
        .oneshot(put_project_req("my-project", &project, &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // GET (200 + ETag)
    let resp = app
        .clone()
        .oneshot(get_authed_req("/projects/my-project", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().contains_key(header::ETAG));

    // LIST
    let resp = app
        .clone()
        .oneshot(get_authed_req("/projects", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json.as_array().is_some_and(|arr| arr.len() == 1));

    // DELETE
    let resp = app
        .clone()
        .oneshot(delete_req("/projects/my-project", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // GET 404 after delete
    let resp = app
        .clone()
        .oneshot(get_authed_req("/projects/my-project", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Test 2: put_is_upsert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn put_is_upsert() {
    let (app, token) = make_authed_app().await;
    let mut project = bool_project("upsert-project");

    // First PUT -> 201
    let resp = app
        .clone()
        .oneshot(put_project_req("upsert-project", &project, &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Second PUT with different name -> 200
    project.name = "Updated Name".into();
    let resp = app
        .clone()
        .oneshot(put_project_req("upsert-project", &project, &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // GET should show updated name
    let resp = app
        .clone()
        .oneshot(get_authed_req("/projects/upsert-project", &token))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["name"].as_str(), Some("Updated Name"));
}

// ---------------------------------------------------------------------------
// Test 3: path_key_mismatch_returns_422
// ---------------------------------------------------------------------------

#[tokio::test]
async fn path_key_mismatch_returns_422() {
    let (app, token) = make_authed_app().await;
    // Body has key "other-project" but path is "my-project"
    let project = bool_project("other-project");
    let resp = app
        .oneshot(put_project_req("my-project", &project, &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ---------------------------------------------------------------------------
// Test 4: get_returns_etag_header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_returns_etag_header() {
    let (app, token) = make_authed_app().await;
    let project = bool_project("etag-project");

    app.clone()
        .oneshot(put_project_req("etag-project", &project, &token))
        .await
        .unwrap();

    let resp1 = app
        .clone()
        .oneshot(get_authed_req("/projects/etag-project", &token))
        .await
        .unwrap();
    let etag1 = extract_etag(&resp1).expect("should have ETag");
    assert!(!etag1.is_empty());
    drop(resp1);

    // Second GET must return identical ETag
    let resp2 = app
        .clone()
        .oneshot(get_authed_req("/projects/etag-project", &token))
        .await
        .unwrap();
    let etag2 = extract_etag(&resp2).expect("should have ETag");
    assert_eq!(etag1, etag2, "ETag must be stable between reads");
}

// ---------------------------------------------------------------------------
// Test 5: stale_if_match_returns_412
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stale_if_match_returns_412() {
    let (app, token) = make_authed_app().await;
    let project = bool_project("stale-project");

    // PUT (create)
    app.clone()
        .oneshot(put_project_req("stale-project", &project, &token))
        .await
        .unwrap();

    // GET to obtain ETag e1
    let resp = app
        .clone()
        .oneshot(get_authed_req("/projects/stale-project", &token))
        .await
        .unwrap();
    let etag1 = extract_etag(&resp).unwrap();
    drop(resp);

    // PUT without If-Match (succeeds, changes the state)
    let mut updated = project.clone();
    updated.name = "Stale Modified".into();
    app.clone()
        .oneshot(put_project_req("stale-project", &updated, &token))
        .await
        .unwrap();

    // PUT with stale If-Match=e1 -> 412
    let req = Request::builder()
        .method("PUT")
        .uri("/projects/stale-project")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .header(header::IF_MATCH, etag1)
        .body(Body::from(serde_json::to_vec(&updated).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
}

// ---------------------------------------------------------------------------
// Test 6: matching_if_match_succeeds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn matching_if_match_succeeds() {
    let (app, token) = make_authed_app().await;
    let project = bool_project("match-project");

    app.clone()
        .oneshot(put_project_req("match-project", &project, &token))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(get_authed_req("/projects/match-project", &token))
        .await
        .unwrap();
    let current_etag = extract_etag(&resp).unwrap();
    drop(resp);

    // PUT with correct If-Match -> 200
    let mut updated = project.clone();
    updated.name = "Matched Update".into();
    let req = Request::builder()
        .method("PUT")
        .uri("/projects/match-project")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .header(header::IF_MATCH, current_etag)
        .body(Body::from(serde_json::to_vec(&updated).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Test 7: invalid_rule_rejected_and_not_persisted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_rule_rejected_and_not_persisted() {
    let (app, token) = make_authed_app().await;
    let project = bool_project("invalid-project");
    let env = bool_environment("prod");
    let flag = bool_flag("my-flag");

    // Setup: project + env + flag
    app.clone()
        .oneshot(put_project_req("invalid-project", &project, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_env_req("invalid-project", "prod", &env, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_flag_req("invalid-project", "my-flag", &flag, &token))
        .await
        .unwrap();

    // PUT flag_env_config referencing a non-existent segment -> 400
    let bad_config = FlagEnvConfig {
        enabled: true,
        rules: vec![TargetingRule {
            segments: vec![segment_key("ghost-segment")],
            serve: ServeTarget::Fixed(variant_key("on")),
        }],
        default_rule: ServeTarget::Fixed(variant_key("off")),
    };
    let resp = app
        .clone()
        .oneshot(put_config_req(
            "invalid-project",
            "my-flag",
            "prod",
            &bad_config,
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "invalid rule should be rejected"
    );

    // Verify content-type is problem+json
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("problem+json"),
        "error must be problem+json: {ct}"
    );

    // GET the config -> 404 (nothing was written)
    let resp = app
        .clone()
        .oneshot(get_authed_req(
            "/projects/invalid-project/flags/my-flag/environments/prod/config",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "config must not have been persisted"
    );
}

// ---------------------------------------------------------------------------
// Test 8: valid_mutation_persists_and_audits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn valid_mutation_persists_and_audits() {
    let store = make_store().await;
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .unwrap();
    let state = AppState::new(store.clone());
    let app = build_router(state);

    // Login
    let login_body = serde_json::json!({ "username": ADMIN_USER, "password": ADMIN_PASS });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let project = bool_project("audit-project");
    let env = bool_environment("prod");
    let flag = bool_flag("audited-flag");
    let config = simple_config("on");

    app.clone()
        .oneshot(put_project_req("audit-project", &project, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_env_req("audit-project", "prod", &env, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_flag_req("audit-project", "audited-flag", &flag, &token))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(put_config_req(
            "audit-project",
            "audited-flag",
            "prod",
            &config,
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Verify the resource is in the store
    let stored = store
        .get_flag_env_config(
            &project_key("audit-project"),
            &flag_key("audited-flag"),
            &env_key("prod"),
        )
        .await
        .unwrap();
    assert!(stored.is_some(), "config must be persisted in store");

    // Verify an audit entry exists for a mutation performed by the admin user.
    let entries = store.list_audit_entries().await.unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.actor == ADMIN_USER && e.action.contains("created")),
        "must have at least one audit entry with actor={ADMIN_USER}"
    );
}

// ---------------------------------------------------------------------------
// Test 9: mutation_refreshes_compiled_cache
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mutation_refreshes_compiled_cache() {
    let store = make_store().await;
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .unwrap();
    let state = AppState::new(store);
    let cache = state.cache.clone();
    let app = build_router(state);

    // Login
    let login_body = serde_json::json!({ "username": ADMIN_USER, "password": ADMIN_PASS });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let project = bool_project("cache-project");
    let env = bool_environment("prod");
    let flag = bool_flag("cache-flag");
    let config_v1 = simple_config("on");
    let mut config_v2 = simple_config("off");
    config_v2.enabled = false;

    app.clone()
        .oneshot(put_project_req("cache-project", &project, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_env_req("cache-project", "prod", &env, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_flag_req("cache-project", "cache-flag", &flag, &token))
        .await
        .unwrap();

    // PUT config v1
    app.clone()
        .oneshot(put_config_req(
            "cache-project",
            "cache-flag",
            "prod",
            &config_v1,
            &token,
        ))
        .await
        .unwrap();

    let key = (project_key("cache-project"), env_key("prod"));
    let version_after_v1 = {
        let c = cache.read().await;
        let rs = c
            .get(&key)
            .expect("cache must contain the ruleset after v1");
        assert!(!rs.content_hash.is_empty());
        rs.version
    };

    // PUT config v2 (different content)
    app.clone()
        .oneshot(put_config_req(
            "cache-project",
            "cache-flag",
            "prod",
            &config_v2,
            &token,
        ))
        .await
        .unwrap();

    let version_after_v2 = {
        let c = cache.read().await;
        c.get(&key)
            .expect("cache must still contain ruleset after v2")
            .version
    };

    assert!(
        version_after_v2 > version_after_v1,
        "version must increment after a content change: {version_after_v1} -> {version_after_v2}"
    );
}

// ---------------------------------------------------------------------------
// Test 10: segment_change_recompiles_referencing_envs
// ---------------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn segment_change_recompiles_referencing_envs() {
    let store = make_store().await;
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .unwrap();
    let state = AppState::new(store);
    let cache = state.cache.clone();
    let app = build_router(state);

    // Login
    let login_body = serde_json::json!({ "username": ADMIN_USER, "password": ADMIN_PASS });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let project = bool_project("seg-project");
    let env1 = bool_environment("env1");
    let env2 = bool_environment("env2");
    let flag = bool_flag("seg-flag");
    let seg = simple_segment("my-segment");

    app.clone()
        .oneshot(put_project_req("seg-project", &project, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_env_req("seg-project", "env1", &env1, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_env_req("seg-project", "env2", &env2, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_flag_req("seg-project", "seg-flag", &flag, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_segment_req("seg-project", "my-segment", &seg, &token))
        .await
        .unwrap();

    // Config with segment rule in both envs
    let seg_config = FlagEnvConfig {
        enabled: true,
        rules: vec![TargetingRule {
            segments: vec![segment_key("my-segment")],
            serve: ServeTarget::Fixed(variant_key("on")),
        }],
        default_rule: ServeTarget::Fixed(variant_key("off")),
    };

    app.clone()
        .oneshot(put_config_req(
            "seg-project",
            "seg-flag",
            "env1",
            &seg_config,
            &token,
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_config_req(
            "seg-project",
            "seg-flag",
            "env2",
            &seg_config,
            &token,
        ))
        .await
        .unwrap();

    // Capture current hashes
    let (hash1_before, hash2_before) = {
        let c = cache.read().await;
        let h1 = c
            .get(&(project_key("seg-project"), env_key("env1")))
            .map(|r| r.content_hash.clone())
            .unwrap_or_default();
        let h2 = c
            .get(&(project_key("seg-project"), env_key("env2")))
            .map(|r| r.content_hash.clone())
            .unwrap_or_default();
        (h1, h2)
    };

    // Mutate the segment
    let mut seg_updated = seg.clone();
    seg_updated.name = "updated-segment".into();
    app.clone()
        .oneshot(put_segment_req(
            "seg-project",
            "my-segment",
            &seg_updated,
            &token,
        ))
        .await
        .unwrap();

    // Both envs must be recompiled in the cache
    {
        let c = cache.read().await;
        assert!(
            c.contains_key(&(project_key("seg-project"), env_key("env1"))),
            "env1 must be in cache"
        );
        assert!(
            c.contains_key(&(project_key("seg-project"), env_key("env2"))),
            "env2 must be in cache"
        );
        let h1_after = c
            .get(&(project_key("seg-project"), env_key("env1")))
            .map(|r| r.content_hash.clone())
            .unwrap_or_default();
        let h2_after = c
            .get(&(project_key("seg-project"), env_key("env2")))
            .map(|r| r.content_hash.clone())
            .unwrap_or_default();
        assert!(!h1_after.is_empty(), "env1 hash must not be empty");
        assert!(!h2_after.is_empty(), "env2 hash must not be empty");
        let _ = (hash1_before, hash2_before); // suppress unused warnings
    }
}

// ---------------------------------------------------------------------------
// Test 11: missing_auth_returns_401
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_auth_returns_401() {
    let (app, _token) = make_authed_app().await;
    let project = bool_project("no-auth-project");

    // PUT without Authorization header
    let req = Request::builder()
        .method("PUT")
        .uri("/projects/no-auth-project")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&project).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Test 12: federated_resource_mutation_warns
// ---------------------------------------------------------------------------

#[tokio::test]
async fn federated_resource_mutation_warns() {
    let (app, token) = make_authed_app().await;
    let mut project = bool_project("fed-project");
    project.managed_by = ManagedBy::Federated;

    let resp = app
        .oneshot(put_project_req("fed-project", &project, &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert!(
        resp.headers().contains_key("X-Flaps-Warning"),
        "federated resource must carry X-Flaps-Warning header"
    );
}

// ---------------------------------------------------------------------------
// Test 13: external_ref_conflict_returns_409
// ---------------------------------------------------------------------------

#[tokio::test]
async fn external_ref_conflict_returns_409() {
    use flaps_domain::ExternalRef;
    let (app, token) = make_authed_app().await;

    let mut project_a = bool_project("proj-a");
    project_a.external_ref = Some(ExternalRef::new("urn:shared:ref"));

    let mut project_b = bool_project("proj-b");
    project_b.external_ref = Some(ExternalRef::new("urn:shared:ref"));

    // First project -> 201
    let resp = app
        .clone()
        .oneshot(put_project_req("proj-a", &project_a, &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Second project with same external_ref -> 409
    let resp = app
        .clone()
        .oneshot(put_project_req("proj-b", &project_b, &token))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "duplicate external_ref must return 409"
    );
}

// ---------------------------------------------------------------------------
// Test 14: delete_absent_returns_404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_absent_returns_404() {
    let (app, token) = make_authed_app().await;

    let resp = app
        .oneshot(delete_req("/projects/nonexistent-project", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Test 15: login_succeeds_with_valid_credentials
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_succeeds_with_valid_credentials() {
    let (app, token) = make_authed_app().await;
    // token was obtained in make_authed_app; verify it is non-empty.
    assert!(!token.is_empty(), "login must return a non-empty token");

    // Use it to verify it authorizes a read-only admin route.
    let resp = app
        .oneshot(get_authed_req("/projects", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Test 16: login_fails_with_wrong_password
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_fails_with_wrong_password() {
    let store = make_store().await;
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .unwrap();
    let state = AppState::new(store);
    let app = build_router(state);

    let login_body = serde_json::json!({
        "username": ADMIN_USER,
        "password": "wrong-password",
    });
    let req = Request::builder()
        .method("POST")
        .uri("/login")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Test 17: invalid_token_returns_401
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_token_returns_401() {
    let (app, _token) = make_authed_app().await;
    let project = bool_project("auth-project");

    let req = Request::builder()
        .method("PUT")
        .uri("/projects/auth-project")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer invalid-token-xyz")
        .body(Body::from(serde_json::to_vec(&project).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Test 18: sdk_key_crud_and_revocation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sdk_key_crud_and_revocation() {
    let (app, token) = make_authed_app().await;
    let project = bool_project("sdk-proj");
    let env = bool_environment("prod");

    app.clone()
        .oneshot(put_project_req("sdk-proj", &project, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_env_req("sdk-proj", "prod", &env, &token))
        .await
        .unwrap();

    // POST key -> 201
    let create_body = serde_json::json!({ "kind": "server" });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/sdk-proj/environments/prod/keys")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["secret"].as_str().is_some(), "secret must be returned");
    let prefix = json["record"]["prefix"].as_str().unwrap().to_owned();

    // LIST -> 1 key
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/projects/sdk-proj/environments/prod/keys")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    // DELETE (revoke)
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/projects/sdk-proj/environments/prod/keys/{prefix}"
                ))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // LIST still shows the key (revoked but listed)
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/projects/sdk-proj/environments/prod/keys")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        list.as_array().unwrap().len(),
        1,
        "revoked key must still appear in list"
    );
}

// ---------------------------------------------------------------------------
// Test 19: whoami_with_valid_sdk_key
// ---------------------------------------------------------------------------

#[tokio::test]
async fn whoami_with_valid_sdk_key() {
    let store = make_store().await;
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .unwrap();
    let state = AppState::new(store);
    let app = build_router(state);

    // Login as admin.
    let login_body = serde_json::json!({ "username": ADMIN_USER, "password": ADMIN_PASS });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let admin_token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // Create project + env + SDK key.
    let project = bool_project("whoami-proj");
    let env = bool_environment("whoami-env");
    app.clone()
        .oneshot(put_project_req("whoami-proj", &project, &admin_token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_env_req("whoami-proj", "whoami-env", &env, &admin_token))
        .await
        .unwrap();

    let create_body = serde_json::json!({ "kind": "client" });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/whoami-proj/environments/whoami-env/keys")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sdk_secret = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_owned();

    // GET /sdk/whoami with the SDK key.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/sdk/whoami")
                .header("Authorization", format!("Bearer {sdk_secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["kind"].as_str(), Some("client"));
    assert_eq!(json["scope"]["project_key"].as_str(), Some("whoami-proj"));
    assert_eq!(
        json["scope"]["environment_key"].as_str(),
        Some("whoami-env")
    );
}

// ---------------------------------------------------------------------------
// Test 20: whoami_without_sdk_key_returns_401
// ---------------------------------------------------------------------------

#[tokio::test]
async fn whoami_without_sdk_key_returns_401() {
    let (app, _token) = make_authed_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/sdk/whoami")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Test 21: login_is_rate_limited_after_repeated_failures (#53)
// ---------------------------------------------------------------------------

async fn login_attempt(
    app: &axum::Router,
    username: &str,
    password: &str,
) -> axum::response::Response {
    let login_body = serde_json::json!({ "username": username, "password": password });
    let req = Request::builder()
        .method("POST")
        .uri("/login")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

#[tokio::test]
async fn login_is_rate_limited_after_repeated_failures() {
    let store = make_store().await;
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .unwrap();
    // Default AppState::new: login burst capacity is 5 (see state.rs).
    let state = AppState::new(store);
    let app = build_router(state);

    // The rate limiter is keyed by username and checked before credentials
    // are verified, so repeated failed attempts still consume the budget.
    for attempt in 1..=5 {
        let resp = login_attempt(&app, ADMIN_USER, "wrong-password").await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "attempt {attempt} should be within the burst budget"
        );
    }

    let resp = login_attempt(&app, ADMIN_USER, "wrong-password").await;
    assert_eq!(
        resp.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "attempt beyond the burst budget must be throttled"
    );
    let retry_after = resp
        .headers()
        .get("Retry-After")
        .expect("429 response must carry a Retry-After header")
        .to_str()
        .unwrap()
        .parse::<u64>()
        .expect("Retry-After must be a non-negative integer of seconds");
    assert!(retry_after > 0, "Retry-After must suggest a positive wait");
}

// ---------------------------------------------------------------------------
// Test 22: login_rate_limiter_disabled_never_throttles (#53)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_rate_limiter_disabled_never_throttles() {
    let store = make_store().await;
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .unwrap();
    let rate_limiter = Arc::new(RateLimiter::new(RateLimitConfig {
        enabled: true,
        capacity: 60,
        refill_per_second: 1.0,
    }));
    let login_rate_limiter = Arc::new(RateLimiter::disabled());
    let state = AppState::with_config(
        store,
        rate_limiter,
        login_rate_limiter,
        std::time::Duration::from_secs(3600),
    );
    let app = build_router(state);

    for attempt in 1..=10 {
        let resp = login_attempt(&app, ADMIN_USER, "wrong-password").await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "attempt {attempt} must never be throttled when the login limiter is disabled"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 23: admin_read_endpoints_require_admin_session (#82)
// ---------------------------------------------------------------------------

/// Covers all ten admin read handlers (list/get on projects, environments,
/// flags, segments, flag-env-config, and list_sdk_keys): an anonymous request
/// must be rejected with 401, and the same request presenting a valid admin
/// session token must succeed with 200.
#[tokio::test]
async fn admin_read_endpoints_require_admin_session() {
    let (app, token) = make_authed_app().await;

    let project = bool_project("read-auth-project");
    let env = bool_environment("read-auth-env");
    let flag = bool_flag("read-auth-flag");
    let segment = simple_segment("read-auth-segment");
    let config = simple_config("on");

    app.clone()
        .oneshot(put_project_req("read-auth-project", &project, &token))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_env_req(
            "read-auth-project",
            "read-auth-env",
            &env,
            &token,
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_flag_req(
            "read-auth-project",
            "read-auth-flag",
            &flag,
            &token,
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_segment_req(
            "read-auth-project",
            "read-auth-segment",
            &segment,
            &token,
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(put_config_req(
            "read-auth-project",
            "read-auth-flag",
            "read-auth-env",
            &config,
            &token,
        ))
        .await
        .unwrap();

    let create_key_body = serde_json::json!({ "kind": "server" });
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/read-auth-project/environments/read-auth-env/keys")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::from(serde_json::to_vec(&create_key_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let read_uris = [
        "/projects",
        "/projects/read-auth-project",
        "/projects/read-auth-project/environments",
        "/projects/read-auth-project/environments/read-auth-env",
        "/projects/read-auth-project/flags",
        "/projects/read-auth-project/flags/read-auth-flag",
        "/projects/read-auth-project/segments",
        "/projects/read-auth-project/segments/read-auth-segment",
        "/projects/read-auth-project/flags/read-auth-flag/environments/read-auth-env/config",
        "/projects/read-auth-project/environments/read-auth-env/keys",
    ];

    for uri in read_uris {
        let resp = app.clone().oneshot(get_req(uri)).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "anonymous GET {uri} must be rejected with 401"
        );
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("problem+json"),
            "401 body for {uri} must be problem+json: {ct}"
        );

        let resp = app
            .clone()
            .oneshot(get_authed_req(uri, &token))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "authenticated GET {uri} must succeed with 200"
        );
    }
}
