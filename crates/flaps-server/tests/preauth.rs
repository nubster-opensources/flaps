//! Integration tests for the pre-authentication hardening of `POST /login`
//! and the SDK key extractor (see issues #133 and #134).

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use flaps_server::{
    bootstrap_admin, build_router,
    state::{AppState, DEFAULT_PREAUTH_PER_CLIENT_CAPACITY},
};
use flaps_store::{hash::KeyHasher, sqlite::SqliteStore};
use tower::ServiceExt as _;

const ADMIN_USER: &str = "preauth-admin";
const ADMIN_PASS: &str = "preauth-admin-password";

async fn make_app() -> axum::Router {
    let hasher = KeyHasher::new(b"00000000000000000000000000000000".to_vec());
    let store = SqliteStore::in_memory(hasher)
        .await
        .expect("in-memory store");
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .expect("bootstrap admin");
    build_router(AppState::new(store))
}

fn login_request(username: &str, password: &str) -> Request<Body> {
    let body = serde_json::json!({ "username": username, "password": password });
    Request::builder()
        .method("POST")
        .uri("/login")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("login request")
}

#[tokio::test]
async fn oversized_username_is_rejected_before_any_credential_work() {
    let app = make_app().await;
    let username = "a".repeat(257);

    let response = app
        .oneshot(login_request(&username, ADMIN_PASS))
        .await
        .expect("router response");

    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a username above the accepted bound must be refused"
    );
}

#[tokio::test]
async fn oversized_password_is_rejected_before_any_credential_work() {
    let app = make_app().await;
    let password = "b".repeat(1025);

    let response = app
        .oneshot(login_request(ADMIN_USER, &password))
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn oversized_login_body_is_rejected_by_the_route_limit() {
    let app = make_app().await;
    let filler = "c".repeat(8 * 1024);

    let response = app
        .oneshot(login_request(ADMIN_USER, &filler))
        .await
        .expect("router response");

    assert!(
        response.status() == StatusCode::PAYLOAD_TOO_LARGE
            || response.status() == StatusCode::UNPROCESSABLE_ENTITY,
        "a body beyond the route limit must never reach credential verification, got {}",
        response.status()
    );
}

#[tokio::test]
async fn a_normal_login_still_succeeds() {
    let app = make_app().await;

    let response = app
        .oneshot(login_request(ADMIN_USER, ADMIN_PASS))
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn rotating_usernames_are_throttled_at_the_login_route() {
    // Without the address layer, each fresh username starts with a full
    // bucket and this flood never meets any resistance.
    let app = make_app().await;
    let mut statuses = Vec::new();

    for attempt in 0..64 {
        let response = app
            .clone()
            .oneshot(login_request(
                &format!("nobody-{attempt}"),
                "wrong-password",
            ))
            .await
            .expect("router response");
        statuses.push(response.status());
    }

    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "a flood of rotating usernames must be throttled, got {statuses:?}"
    );
}

#[tokio::test]
async fn a_throttled_login_advertises_a_retry_delay() {
    let app = make_app().await;

    let mut throttled = None;
    for attempt in 0..64 {
        let response = app
            .clone()
            .oneshot(login_request(
                &format!("nobody-{attempt}"),
                "wrong-password",
            ))
            .await
            .expect("router response");
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            throttled = Some(response);
            break;
        }
    }

    let response = throttled.expect("the flood must eventually be throttled");
    assert!(
        response.headers().contains_key("retry-after"),
        "a refusal must tell the caller when to come back"
    );
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json"),
        "the refusal must use the documented error format"
    );
}

#[tokio::test]
async fn a_login_burst_does_not_starve_unrelated_requests() {
    // The point of moving Argon2 off the runtime: a burst of logins must not
    // freeze requests that have nothing to do with authentication.
    let app = make_app().await;

    let burst = (0..30).map(|attempt| {
        let app = app.clone();
        tokio::spawn(async move {
            app.oneshot(login_request(
                &format!("nobody-{attempt}"),
                "wrong-password",
            ))
            .await
        })
    });

    let unrelated = tokio::spawn({
        let app = app.clone();
        async move {
            let request = Request::builder()
                .method("GET")
                .uri("/projects")
                .body(Body::empty())
                .expect("request");
            app.oneshot(request).await
        }
    });

    let answered = tokio::time::timeout(std::time::Duration::from_secs(5), unrelated)
        .await
        .expect("an unrelated request must not be starved by a login burst")
        .expect("join")
        .expect("router response");

    assert_eq!(
        answered.status(),
        StatusCode::UNAUTHORIZED,
        "the unrelated request is answered on its own merits, not delayed by the burst"
    );

    for task in burst {
        let _ = task.await;
    }
}

#[tokio::test]
async fn impossible_sdk_keys_never_reach_the_database() {
    let hasher = KeyHasher::new(b"00000000000000000000000000000000".to_vec());
    let store = SqliteStore::in_memory(hasher).await.expect("store");
    let app = build_router(AppState::new(store.clone()));

    let before = store.sdk_key_lookups();

    for attempt in 0..50 {
        let request = Request::builder()
            .method("GET")
            .uri("/sdk/whoami")
            .header("Authorization", format!("Bearer garbage-{attempt}"))
            .body(Body::empty())
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    assert_eq!(
        store.sdk_key_lookups(),
        before,
        "a flood of impossible credentials must not produce one query per request"
    );
}

#[tokio::test]
async fn impossible_and_absent_keys_look_the_same() {
    // Any observable difference turns the status code into an oracle of key
    // validity, which is exactly what an enumeration attack needs.
    let app = make_app().await;

    let impossible = whoami_status_and_body(&app, "garbage").await;
    let absent = whoami_status_and_body(&app, &format!("sv_{}", "ab".repeat(24))).await;

    assert_eq!(impossible.0, absent.0, "status codes must match");
    assert_eq!(impossible.1, absent.1, "problem bodies must match");
}

/// Returns the status and the parsed problem body of a `GET /sdk/whoami`
/// attempt carrying `key`.
async fn whoami_status_and_body(app: &axum::Router, key: &str) -> (StatusCode, serde_json::Value) {
    use http_body_util::BodyExt as _;

    let request = Request::builder()
        .method("GET")
        .uri("/sdk/whoami")
        .header("Authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .expect("request");

    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let parsed = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);

    (status, parsed)
}

// ---------------------------------------------------------------------------
// Option A (issue #134): budget SDK key lookups on failure only
// ---------------------------------------------------------------------------

async fn admin_login(app: &axum::Router) -> String {
    use http_body_util::BodyExt as _;

    let resp = app
        .clone()
        .oneshot(login_request(ADMIN_USER, ADMIN_PASS))
        .await
        .expect("router response");
    assert_eq!(resp.status(), StatusCode::OK, "admin login must succeed");
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("login body");
    json["token"].as_str().expect("token field").to_owned()
}

async fn create_project(app: &axum::Router, key: &str, token: &str) {
    let body = serde_json::json!({"key": key, "name": key, "managed_by": "local"});
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/projects/{key}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("response");
    assert!(
        response.status().is_success(),
        "create project must succeed: {}",
        response.status()
    );
}

async fn create_environment(app: &axum::Router, project: &str, environment: &str, token: &str) {
    let body = serde_json::json!({"key": environment, "name": environment, "managed_by": "local"});
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/projects/{project}/environments/{environment}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("response");
    assert!(
        response.status().is_success(),
        "create environment must succeed: {}",
        response.status()
    );
}

/// Creates an SDK key and returns the raw secret (returned once at creation).
async fn create_sdk_key(
    app: &axum::Router,
    project: &str,
    environment: &str,
    token: &str,
) -> String {
    use http_body_util::BodyExt as _;

    let body = serde_json::json!({"kind": "server"});
    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "/projects/{project}/environments/{environment}/keys"
        ))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("response");
    assert!(
        response.status().is_success(),
        "create sdk key must succeed: {}",
        response.status()
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("sdk key body");
    json["secret"].as_str().expect("secret field").to_owned()
}

/// A valid SDK key never spends the pre-authentication budget (issue #134,
/// Option A): the budget is only charged on a FAILED lookup, so a legitimate
/// SDK client hammering the API from one address with one key is never
/// throttled, however many requests it makes.
#[tokio::test]
async fn a_valid_sdk_key_is_never_throttled_by_the_preauth_budget() {
    let app = make_app().await;

    let token = admin_login(&app).await;
    create_project(&app, "test-proj", &token).await;
    create_environment(&app, "test-proj", "test-env", &token).await;
    let sdk_key = create_sdk_key(&app, "test-proj", "test-env", &token).await;

    for attempt in 0..50 {
        let request = Request::builder()
            .method("GET")
            .uri("/sdk/whoami")
            .header("Authorization", format!("Bearer {sdk_key}"))
            .body(Body::empty())
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "attempt {attempt} with a valid key must never be throttled"
        );
    }
}

/// A flood of well-formed but absent SDK keys is bounded before the database:
/// once the wide layers (global, per-client) are exhausted, further failed
/// lookups are refused by the budget peek, before `find_sdk_key` runs.
#[tokio::test]
async fn a_flood_of_wellformed_but_absent_keys_stops_hitting_the_database() {
    let hasher = KeyHasher::new(b"00000000000000000000000000000000".to_vec());
    let store = SqliteStore::in_memory(hasher).await.expect("store");
    bootstrap_admin(&store, ADMIN_USER, ADMIN_PASS)
        .await
        .expect("bootstrap admin");
    let app = build_router(AppState::new(store.clone()));

    let before = store.sdk_key_lookups();

    for attempt in 0..60 {
        let key = format!("sv_{attempt:048x}");
        let request = Request::builder()
            .method("GET")
            .uri("/sdk/whoami")
            .header("Authorization", format!("Bearer {key}"))
            .body(Body::empty())
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("response");
        assert!(
            response.status() == StatusCode::UNAUTHORIZED
                || response.status() == StatusCode::TOO_MANY_REQUESTS,
            "a well-formed but absent key must be refused, got {}",
            response.status()
        );
    }

    let lookups_added = store.sdk_key_lookups() - before;
    // The binding layer is now the per-client budget on the Unknown bucket
    // (all 60 requests share one address: the test never sets connection
    // info), so lookups can only happen while that bucket still has
    // capacity. The margin of 2 absorbs the trickle refill
    // (DEFAULT_PREAUTH_PER_CLIENT_REFILL_PER_SECOND = 1.0/s) that can grant
    // at most one or two extra tokens over the wall-clock time 60 sequential
    // in-memory requests take to run; it was picked by running this test
    // repeatedly and stayed at exactly DEFAULT_PREAUTH_PER_CLIENT_CAPACITY
    // lookups every time, so 2 leaves headroom without loosening the bound
    // back toward "less than one per request".
    let max_expected_lookups = u64::from(DEFAULT_PREAUTH_PER_CLIENT_CAPACITY) + 2;
    assert!(
        lookups_added <= max_expected_lookups,
        "a flood of well-formed but absent keys must stop at the per-client budget, \
         got {lookups_added} lookups for 60 requests (expected at most {max_expected_lookups})"
    );
}
