//! Integration tests for the pre-authentication hardening of `POST /login`
//! and the SDK key extractor (see issues #133 and #134).

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use flaps_server::{bootstrap_admin, build_router, state::AppState};
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
