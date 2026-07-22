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
use flaps_server::{
    bootstrap_admin, build_router,
    state::{AppState, Store},
};
use flaps_store::{hash::KeyHasher, postgres::PostgresStore, sqlite::SqliteStore};
use http_body_util::BodyExt;
use tokio::sync::Barrier;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test helpers (deliberately duplicated from tests/admin_api.rs: each
// integration test file is its own crate).
// ---------------------------------------------------------------------------

const ADMIN_USER: &str = "test-admin";
const ADMIN_PASS: &str = "test-admin-password";

async fn make_sqlite_store() -> SqliteStore {
    let hasher = KeyHasher::new(b"00000000000000000000000000000000".to_vec());
    SqliteStore::in_memory(hasher)
        .await
        .expect("in-memory store")
}

/// Connects to the PostgreSQL instance named by `FLAPS_TEST_POSTGRES_URL`,
/// or `None` if the variable is unset (local development without Postgres).
/// Mirrors the skip-silently convention used by `crates/flaps-store/tests/postgres.rs`.
async fn maybe_make_postgres_store() -> Option<PostgresStore> {
    let url = std::env::var("FLAPS_TEST_POSTGRES_URL").ok()?;
    let hasher = KeyHasher::new(b"concurrency-test-pepper-32-bytes".to_vec());
    Some(
        PostgresStore::connect(&url, hasher)
            .await
            .expect("connect to FLAPS_TEST_POSTGRES_URL"),
    )
}

/// Builds an app, its `AppState` (for direct cache inspection) and a valid
/// admin session token around an already-constructed store, generic over the
/// backend so the same test body proves SQLite and PostgreSQL equivalent
/// (issue #108's "SQLite and PostgreSQL provide equivalent behavior").
async fn make_authed_app<S: Store>(store: S) -> (axum::Router, AppState<S>, String) {
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

fn delete_req(uri: &str, token: &str, if_match: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"));
    if let Some(etag) = if_match {
        builder = builder.header(
            header::IF_MATCH,
            HeaderValue::from_str(etag).expect("valid header value"),
        );
    }
    builder.body(Body::empty()).unwrap()
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

/// Store-agnostic body: proves #108 for any `S: Store` backend.
async fn assert_concurrent_put_flag_same_etag_yields_exactly_one_success<S: Store>(store: S) {
    let (app, _state, token) = make_authed_app(store).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_put_flag_same_etag_yields_exactly_one_success_sqlite() {
    assert_concurrent_put_flag_same_etag_yields_exactly_one_success(make_sqlite_store().await)
        .await;
}

/// CI-only mirror of the SQLite race test above, proving Postgres behaves
/// identically. Skips silently when `FLAPS_TEST_POSTGRES_URL` is unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_put_flag_same_etag_yields_exactly_one_success_postgres() {
    let Some(store) = maybe_make_postgres_store().await else {
        return;
    };
    assert_concurrent_put_flag_same_etag_yields_exactly_one_success(store).await;
}

// ---------------------------------------------------------------------------
// #105: two concurrent mutations to DISTINCT flags in one environment must
// leave BOTH changes in the cached ruleset, with a strictly higher version.
// ---------------------------------------------------------------------------

/// Store-agnostic body: proves #105 for any `S: Store` backend.
async fn assert_concurrent_put_flag_env_config_distinct_flags_both_present_in_cache<S: Store>(
    store: S,
) {
    let (app, state, token) = make_authed_app(store).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_put_flag_env_config_distinct_flags_both_present_in_cache_sqlite() {
    assert_concurrent_put_flag_env_config_distinct_flags_both_present_in_cache(
        make_sqlite_store().await,
    )
    .await;
}

/// CI-only mirror of the SQLite race test above, proving Postgres behaves
/// identically. Skips silently when `FLAPS_TEST_POSTGRES_URL` is unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_put_flag_env_config_distinct_flags_both_present_in_cache_postgres() {
    let Some(store) = maybe_make_postgres_store().await else {
        return;
    };
    assert_concurrent_put_flag_env_config_distinct_flags_both_present_in_cache(store).await;
}

// ---------------------------------------------------------------------------
// #105 (ordering pin): deleting a flag with an existing flag_env_config must
// evict the flag from the environment's cached ruleset, even though deleting
// the flag cascades and deletes the flag_env_config row that a NAIVE
// post-write "affected environments" lookup would rely on.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_flag_with_env_config_recompiles_env_from_committed_state() {
    let (app, state, token) = make_authed_app(make_sqlite_store().await).await;
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
        .oneshot(delete_req("/projects/proj/flags/doomed-flag", &token, None))
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
    let (app, _state, token) = make_authed_app(make_sqlite_store().await).await;
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
    let (app, _state, token) = make_authed_app(make_sqlite_store().await).await;
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
    let (app, _state, token) = make_authed_app(make_sqlite_store().await).await;
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

// ---------------------------------------------------------------------------
// Fix 3: repeated `If-None-Match` field-lines must be joined (RFC 7230
// SS3.2.2), never collapsed to just the first line.
// ---------------------------------------------------------------------------

/// The dangerous case Fix 3 closes: `If-None-Match: "x"` followed by
/// `If-None-Match: *`. Reading only the first field-line (as `HeaderMap::get`
/// does) would see just `"x"`, take the non-`*` branch, return `Ok(())`, and
/// silently bypass the create-only guard, overwriting a resource the client
/// explicitly asked never to overwrite.
///
/// Joining the two lines per RFC 7230 SS3.2.2 produces `"x", *`, which is
/// NOT the bare `*` this API's create-only guard supports (Fix 5 rejects
/// every non-`*` value, listed or mixed, as an unsupported precondition
/// rather than silently ignoring it) -- so the response is `422`, not `412`.
/// Either way, the important, security-relevant assertion is the same: the
/// overwrite must be REFUSED, not silently allowed through as it was before
/// this fix.
#[tokio::test]
async fn if_none_match_two_line_bypass_is_closed() {
    let (app, _state, token) = make_authed_app(make_sqlite_store().await).await;
    setup_project_env(&app, &token, "proj", "prod").await;

    let flag = bool_flag("guarded-flag");
    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/guarded-flag",
            &flag,
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "the flag must exist");

    let mut updated = flag.clone();
    updated.description = Some("must never be applied".to_owned());

    let req = Request::builder()
        .method("PUT")
        .uri("/projects/proj/flags/guarded-flag")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .header(header::IF_NONE_MATCH, HeaderValue::from_static("\"x\""))
        .header(header::IF_NONE_MATCH, HeaderValue::from_static("*"))
        .body(Body::from(serde_json::to_vec(&updated).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();

    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "the second If-None-Match: * line must still be seen; the overwrite must never silently \
         succeed just because the first line was not '*'"
    );
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "the joined value \"x\", * is not the bare '*' this guard supports, so it is refused \
         as an unsupported precondition (Fix 5), not silently ignored"
    );

    // Confirm the write was in fact refused: the flag must be unchanged.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/projects/proj/flags/guarded-flag")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(
        body["description"],
        serde_json::Value::Null,
        "the create-only guard must not have been bypassed: the flag must be unchanged"
    );
}

// ---------------------------------------------------------------------------
// Fix 4: a precondition header present but not valid ASCII must fail closed
// (422), never be silently treated as absent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_if_match_header_is_rejected_not_silently_ignored() {
    let (app, _state, token) = make_authed_app(make_sqlite_store().await).await;
    setup_project_env(&app, &token, "proj", "prod").await;

    let flag = bool_flag("malformed-header-flag");
    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/malformed-header-flag",
            &flag,
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let mut updated = flag.clone();
    updated.description = Some("must never be applied".to_owned());

    let req = Request::builder()
        .method("PUT")
        .uri("/projects/proj/flags/malformed-header-flag")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .header(
            header::IF_MATCH,
            HeaderValue::from_bytes(b"\xffnot-ascii").unwrap(),
        )
        .body(Body::from(serde_json::to_vec(&updated).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a non-ASCII If-Match value must fail closed (422), never be silently treated as absent \
         and let the write through unconditionally"
    );

    // Confirm the write was in fact refused: the flag must be unchanged.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/projects/proj/flags/malformed-header-flag")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(
        body["description"],
        serde_json::Value::Null,
        "the write must not have gone through"
    );
}

// ---------------------------------------------------------------------------
// Fix 2: the mutation-lock registry must not grow unboundedly when an admin
// mentions many distinct never-created project keys.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mutation_lock_registry_does_not_grow_for_repeated_nonexistent_projects() {
    let (app, state, token) = make_authed_app(make_sqlite_store().await).await;

    for n in 0..25 {
        let resp = app
            .clone()
            .oneshot(put_req(
                &format!("/projects/never-existed-{n}/flags/x"),
                &bool_flag("x"),
                &token,
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "the parent project does not exist"
        );
    }

    assert_eq!(
        state.mutation_lock_registry_len(),
        0,
        "the registry must not retain one entry per distinct never-created project key"
    );
}

// ---------------------------------------------------------------------------
// Fix 7: `If-Match` on DELETE, covered end to end.
// ---------------------------------------------------------------------------

/// (a) `DELETE` of a MISSING resource carrying `If-Match` (specific or `*`)
/// must return 412, not 404: a deliberate, RFC-correct status change (RFC
/// 7232 SS3.1) introduced by this lot, previously untested at the HTTP layer.
#[tokio::test]
async fn delete_of_missing_flag_with_if_match_is_412() {
    let (app, _state, token) = make_authed_app(make_sqlite_store().await).await;
    setup_project_env(&app, &token, "proj", "prod").await;

    let resp = app
        .clone()
        .oneshot(delete_req(
            "/projects/proj/flags/never-existed",
            &token,
            Some("*"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);

    let resp = app
        .clone()
        .oneshot(delete_req(
            "/projects/proj/flags/never-existed",
            &token,
            Some("\"some-etag\""),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
}

/// (b) Two concurrent `DELETE`s sharing one `If-Match` ETag must yield
/// exactly one success and one 412: acceptance criterion #108 says the
/// precondition is "checked atomically with each update AND DELETE", and
/// only the update half had a regression test before this fix.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_deletes_sharing_one_etag_yield_exactly_one_success() {
    let (app, _state, token) = make_authed_app(make_sqlite_store().await).await;
    setup_project_env(&app, &token, "proj", "prod").await;

    let flag = bool_flag("racily-deleted-flag");
    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/racily-deleted-flag",
            &flag,
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let etag = extract_etag(&resp).expect("PUT response must carry an ETag");

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let app = app.clone();
        let token = token.clone();
        let etag = etag.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            app.oneshot(delete_req(
                "/projects/proj/flags/racily-deleted-flag",
                &token,
                Some(&etag),
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
        vec![StatusCode::NO_CONTENT, StatusCode::PRECONDITION_FAILED],
        "exactly one concurrent delete with the same If-Match ETag must succeed (204), \
         the other must observe the resource is already gone and get 412; got {statuses:?}"
    );
}

/// A concurrent `PUT` and `DELETE` sharing one `If-Match` ETag must also
/// yield exactly one success: the other half of #108's DELETE coverage.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_put_and_delete_sharing_one_etag_yield_exactly_one_success() {
    let (app, _state, token) = make_authed_app(make_sqlite_store().await).await;
    setup_project_env(&app, &token, "proj", "prod").await;

    let flag = bool_flag("put-vs-delete-flag");
    let resp = app
        .clone()
        .oneshot(put_req(
            "/projects/proj/flags/put-vs-delete-flag",
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
    updated.description = Some("updated concurrently with a delete".to_owned());

    let barrier = Arc::new(Barrier::new(2));

    let put_app = app.clone();
    let put_token = token.clone();
    let put_etag = etag.clone();
    let put_barrier = barrier.clone();
    let put_handle = tokio::spawn(async move {
        put_barrier.wait().await;
        put_app
            .oneshot(put_req(
                "/projects/proj/flags/put-vs-delete-flag",
                &updated,
                &put_token,
                Some(&put_etag),
                None,
            ))
            .await
            .unwrap()
            .status()
    });

    let delete_app = app.clone();
    let delete_token = token.clone();
    let delete_etag = etag.clone();
    let delete_barrier = barrier.clone();
    let delete_handle = tokio::spawn(async move {
        delete_barrier.wait().await;
        delete_app
            .oneshot(delete_req(
                "/projects/proj/flags/put-vs-delete-flag",
                &delete_token,
                Some(&delete_etag),
            ))
            .await
            .unwrap()
            .status()
    });

    let put_status = put_handle.await.expect("task must not panic");
    let delete_status = delete_handle.await.expect("task must not panic");

    // Either request may win the race (scheduling-dependent, not
    // deterministic): the invariant is exactly one success and one 412, not
    // which of the two operations wins.
    let outcome_is_valid = matches!(
        (put_status, delete_status),
        (StatusCode::OK, StatusCode::PRECONDITION_FAILED)
            | (StatusCode::PRECONDITION_FAILED, StatusCode::NO_CONTENT)
    );

    assert!(
        outcome_is_valid,
        "exactly one of the racing PUT/DELETE sharing the same If-Match ETag must succeed \
         (PUT -> 200 or DELETE -> 204), the other must observe the ETag no longer matches and \
         get 412; got put={put_status}, delete={delete_status}"
    );
}
