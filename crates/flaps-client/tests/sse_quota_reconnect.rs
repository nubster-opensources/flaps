//! Proves the SSE reconnect loop backs off on a quota rejection (issue
//! #111) instead of spinning.
//!
//! `GET /sync/v1/events` responses are turned into `Err` by
//! `reqwest::Response::error_for_status` for ANY non-2xx status, including
//! 429 - so the supervisor's existing "reconnect with full-jitter backoff on
//! error" path (`crates/flaps-client/src/supervisor.rs`) already covers a
//! quota rejection exactly like any other connect failure. This test proves
//! that behaviour end-to-end against a real server whose SSE quota is
//! permanently exhausted (`max_global = 0`), rather than asserting it by
//! reading the source.
//!
//! Throughput over a fixed window (not per-attempt gaps) is the assertion,
//! because full-jitter backoff can legitimately draw a near-zero delay on
//! any single attempt; only the aggregate rate distinguishes "backing off"
//! from "spinning". This is the one test in the flaps-client / flaps-server
//! suites that relies on real wall-clock timing rather than a barrier: a
//! reconnect-throttling property is, by its nature, a statement about
//! elapsed time, not about event ordering.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use flaps_client::{FlapsProvider, FlapsProviderConfig};
use flaps_domain::{Environment, EnvironmentKey, ManagedBy, Project, ProjectKey, SdkKeyKind};
use flaps_server::sse_quota::{SseQuota, SseQuotaConfig};
use flaps_server::state::AppState;
use flaps_store::hash::KeyHasher;
use flaps_store::repository::{
    EnvironmentRepository as _, ProjectRepository as _, SdkKeyRepository as _,
};
use flaps_store::sdk_key::{NewSdkKey, SdkKeyScope};
use flaps_store::sqlite::SqliteStore;
use open_feature::EvaluationContext;
use open_feature::provider::FeatureProvider;
use tokio::net::TcpListener;
use tokio::time::timeout;

const PROJECT: &str = "reconnect-proj";
const ENVIRONMENT: &str = "reconnect-env";
const SDK_SECRET: &str = "s-reconnect-quota-test-server-key-0123456789";

/// Counts `GET /sync/v1/events` requests as they arrive, independent of the
/// response the handler produces.
async fn count_events_requests(counter: Arc<AtomicUsize>, req: Request, next: Next) -> Response {
    if req.uri().path() == "/sync/v1/events" {
        counter.fetch_add(1, Ordering::Relaxed);
    }
    next.run(req).await
}

/// Spawns a real Flaps server whose SSE concurrency quota is permanently
/// exhausted (`max_global = 0`): every `GET /sync/v1/events` is rejected
/// with 429, while `GET /sync/v1/ruleset` behaves normally. Returns the
/// listening address and a live counter of SSE connection attempts.
async fn spawn_quota_exhausted_server() -> (SocketAddr, Arc<AtomicUsize>) {
    let store = SqliteStore::in_memory(KeyHasher::new(b"reconnect-quota-test-pepper-32by!"))
        .await
        .expect("in-memory store");

    let project_key = ProjectKey::new(PROJECT).expect("valid project key");
    let env_key = EnvironmentKey::new(ENVIRONMENT).expect("valid environment key");

    store
        .upsert_project(
            "system",
            &Project {
                key: project_key.clone(),
                name: "reconnect project".into(),
                description: None,
                external_ref: None,
                managed_by: ManagedBy::Local,
            },
        )
        .await
        .expect("upsert project");

    store
        .upsert_environment(
            "system",
            &project_key,
            &Environment {
                key: env_key.clone(),
                name: "reconnect environment".into(),
                external_ref: None,
                managed_by: ManagedBy::Local,
                metadata: flaps_domain::Metadata::new(),
            },
        )
        .await
        .expect("upsert environment");

    store
        .create_sdk_key(
            "system",
            SDK_SECRET,
            &NewSdkKey {
                kind: SdkKeyKind::Server,
                scope: SdkKeyScope {
                    project_key: project_key.clone(),
                    environment_key: env_key.clone(),
                },
            },
        )
        .await
        .expect("create sdk key");

    // Compiles the (empty) ruleset so GET /sync/v1/ruleset returns 200,
    // isolating the quota rejection to the SSE endpoint specifically.
    let state = AppState::new(store).with_sse_quota(Arc::new(SseQuota::new(SseQuotaConfig {
        max_global: 0,
        max_per_key: 0,
    })));
    flaps_server::recompile::recompile_environment(&state, &project_key, &env_key)
        .await
        .expect("initial compile");

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_for_layer = Arc::clone(&counter);

    let app = flaps_server::build_router(state).layer(middleware::from_fn(move |req, next| {
        let counter = Arc::clone(&counter_for_layer);
        count_events_requests(counter, req, next)
    }));

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral listener");
    let addr = listener.local_addr().expect("listener local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("axum serve");
    });

    (addr, counter)
}

#[tokio::test]
async fn quota_rejection_backs_off_instead_of_spinning() {
    let (addr, counter) = spawn_quota_exhausted_server().await;

    let config = FlapsProviderConfig {
        base_url: format!("http://{addr}"),
        sdk_key: SDK_SECRET.to_owned(),
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(5),
        snapshot_path: None,
        staleness_threshold: None,
        // Long enough that any observed SSE attempts are exclusively from
        // the reconnect-on-error path, never from the polling fallback.
        poll_interval: Duration::from_secs(3600),
        backoff_base: Duration::from_millis(30),
        backoff_max: Duration::from_millis(80),
    };

    let mut provider = FlapsProvider::new(config);
    let ctx = EvaluationContext::default();
    timeout(Duration::from_secs(10), provider.initialize(&ctx))
        .await
        .expect("initialize timed out");

    // The initial ruleset fetch (GET /sync/v1/ruleset) succeeds; the
    // background supervisor then repeatedly tries and fails to open the SSE
    // stream. Let it run for a fixed window and measure throughput.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let attempts = counter.load(Ordering::Relaxed);

    assert!(
        attempts >= 2,
        "expected at least a couple of reconnect attempts within 500ms, got {attempts}"
    );

    // A tight loop (no backoff) would drive many hundreds to thousands of
    // attempts against a local, near-instantly-rejecting server within
    // 500ms. With backoff_base=30ms / backoff_max=80ms, a healthy backoff
    // schedule produces on the order of ten attempts. The bound below is
    // deliberately generous (an order of magnitude above the expected
    // count) so the assertion is robust to full-jitter randomness and CI
    // scheduling noise while still failing decisively on a tight loop.
    assert!(
        attempts < 100,
        "reconnect attempts must be throttled by backoff, not a tight loop: \
         {attempts} attempts against a rejecting server in 500ms"
    );
}
