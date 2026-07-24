//! Proves the regression at the heart of issue #111's follow-up: a client
//! whose SSE subscription is PERMANENTLY rejected by the server's
//! concurrency quota (`max_global = 0`, see `flaps_server::sse_quota`) must
//! still observe ruleset changes through the periodic polling fallback in
//! `crates/flaps-client/src/supervisor.rs`.
//!
//! Before the fix, the polling `interval` was constructed only inside the
//! `Ok(stream)` arm of the SSE connect attempt, so a client that never
//! managed to open the stream never polled either: it served the ruleset
//! captured at `initialize()` for the life of the process, silently, since
//! `staleness_threshold` is `None` by default. This test drives the real
//! chain end to end (real server, real quota, real HTTP) rather than
//! asserting the fix by reading the source.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use flaps_client::{FlapsProvider, FlapsProviderConfig};
use flaps_domain::{
    Environment, EnvironmentKey, Flag, FlagEnvConfig, FlagKey, FlagType, ManagedBy, Project,
    ProjectKey, SdkKeyKind, ServeTarget, ValueType, VariantKey, VariantValue, Variants,
};
use flaps_server::sse_quota::{SseQuota, SseQuotaConfig};
use flaps_server::state::AppState;
use flaps_store::hash::KeyHasher;
use flaps_store::repository::{
    EnvironmentRepository as _, FlagEnvConfigRepository as _, FlagRepository as _,
    ProjectRepository as _, SdkKeyRepository as _,
};
use flaps_store::sdk_key::{NewSdkKey, SdkKeyScope};
use flaps_store::sqlite::SqliteStore;
use open_feature::EvaluationContext;
use open_feature::provider::FeatureProvider;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::time::timeout;

const PROJECT: &str = "polling-fallback-proj";
const ENVIRONMENT: &str = "polling-fallback-env";
const FLAG: &str = "polling-fallback-flag";
const ADMIN_PASSWORD: &str = "admin-pass";
// Well-formed: matches the shape `reject_impossible_sdk_key` accepts (see
// issue #134), so this fixture reaches the store instead of being refused
// before it.
const SDK_SECRET: &str = "sv_5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e";

/// A running Flaps server on a real ephemeral socket, seeded with a single
/// boolean flag active in one environment, whose SSE concurrency quota is
/// permanently exhausted (`max_global = 0`): every `GET /sync/v1/events` is
/// rejected with 429, while `GET /sync/v1/ruleset` behaves normally.
struct ServerHandle {
    addr: SocketAddr,
}

/// Spawns [`ServerHandle`], seeded via the store directly with one project,
/// one environment, one boolean flag (enabled, serving `on` = `true`) and one
/// server SDK key.
async fn spawn_quota_exhausted_server() -> ServerHandle {
    let store = SqliteStore::in_memory(KeyHasher::new(b"polling-fallback-test-pepper-32b"))
        .await
        .expect("in-memory store");

    flaps_server::bootstrap_admin(&store, "admin", ADMIN_PASSWORD)
        .await
        .expect("bootstrap admin");

    let project_key = ProjectKey::new(PROJECT).expect("valid project key");
    let env_key = EnvironmentKey::new(ENVIRONMENT).expect("valid environment key");
    let flag_key = FlagKey::new(FLAG).expect("valid flag key");
    let vk_on = VariantKey::new("on").expect("valid variant key");
    let vk_off = VariantKey::new("off").expect("valid variant key");

    seed_active_boolean_flag(&store, &project_key, &env_key, &flag_key, &vk_on, &vk_off).await;

    // The permanently-exhausted SSE quota is the crux of the test: every
    // subscription attempt, from any key, is rejected with 429.
    let state = AppState::new(store).with_sse_quota(Arc::new(SseQuota::new(SseQuotaConfig {
        max_global: 0,
        max_per_key: 0,
    })));

    // The default pre-authentication budget is used here (see issue #134
    // Option A): a valid SDK key never consumes it, since it is only spent on
    // a FAILED key lookup, so this test's own SSE reconnect attempts and
    // 50ms polling cadence on the same valid key never touch it.

    flaps_server::recompile::recompile_environment(&state, &project_key, &env_key)
        .await
        .expect("initial compile");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral listener");
    let addr = listener.local_addr().expect("listener local addr");
    let app = flaps_server::build_router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("axum serve");
    });

    ServerHandle { addr }
}

/// Seeds `store` with one project, one environment, one boolean flag (enabled,
/// serving `on` = `true`) and one server SDK key scoped to that project and
/// environment.
async fn seed_active_boolean_flag(
    store: &SqliteStore,
    project_key: &ProjectKey,
    env_key: &EnvironmentKey,
    flag_key: &FlagKey,
    on: &VariantKey,
    off: &VariantKey,
) {
    store
        .upsert_project(
            "system",
            &Project {
                key: project_key.clone(),
                name: "polling fallback project".into(),
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
            project_key,
            &Environment {
                key: env_key.clone(),
                name: "polling fallback environment".into(),
                external_ref: None,
                managed_by: ManagedBy::Local,
                metadata: flaps_domain::Metadata::new(),
            },
        )
        .await
        .expect("upsert environment");

    let variants = Variants::new(
        ValueType::Boolean,
        [
            (on.clone(), VariantValue::Bool(true)),
            (off.clone(), VariantValue::Bool(false)),
        ],
    )
    .expect("valid variants");

    store
        .upsert_flag(
            "system",
            project_key,
            &Flag {
                key: flag_key.clone(),
                name: "Polling fallback flag".into(),
                description: None,
                flag_type: FlagType::Ops,
                value_type: ValueType::Boolean,
                variants,
                metadata: flaps_domain::Metadata::new(),
            },
        )
        .await
        .expect("upsert flag");

    store
        .upsert_flag_env_config(
            "system",
            project_key,
            flag_key,
            env_key,
            &FlagEnvConfig {
                enabled: true,
                rules: vec![],
                default_rule: ServeTarget::Fixed(on.clone()),
            },
        )
        .await
        .expect("upsert flag env config");

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
}

/// Logs into the admin API and returns the bearer token.
async fn admin_login(addr: SocketAddr, password: &str) -> String {
    let client = reqwest::Client::new();
    let body = serde_json::json!({"username": "admin", "password": password});
    let resp = client
        .post(format!("http://{addr}/login"))
        .json(&body)
        .send()
        .await
        .expect("login request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "admin login must succeed"
    );
    let payload: serde_json::Value = resp.json().await.expect("login response body");
    payload["token"]
        .as_str()
        .expect("token field present in login response")
        .to_owned()
}

/// Sends the real admin `PUT .../config` request that flips the flag's
/// served value.
async fn put_flag_env_config(
    addr: SocketAddr,
    token: &str,
    project: &str,
    flag: &str,
    env: &str,
    body: &serde_json::Value,
) {
    let client = reqwest::Client::new();
    let resp = client
        .put(format!(
            "http://{addr}/projects/{project}/flags/{flag}/environments/{env}/config"
        ))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .expect("put config request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "PUT config must return 200 OK for an update to an existing config"
    );
}

/// Polls `resolve_bool_value` until it succeeds, or panics once `budget` has
/// elapsed. Returns the resolved value.
///
/// `initialize()` only spawns the background supervisor; the very first sync
/// is asynchronous, so a resolve attempted immediately after `initialize()`
/// can race it and observe `ProviderNotReady`. Used both to wait out that
/// initial race and, later, to observe the polled-in ruleset change.
async fn wait_for_resolved_value(
    provider: &FlapsProvider,
    ctx: &EvaluationContext,
    budget: Duration,
) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(result) = provider.resolve_bool_value(FLAG, ctx).await {
            return result.value;
        }
        assert!(
            start.elapsed() < budget,
            "flag never resolved within {budget:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn quota_rejected_client_still_observes_ruleset_change_via_polling() {
    let handle = spawn_quota_exhausted_server().await;

    // A short poll interval so the fallback's cadence, not real time, bounds
    // the test. `backoff_base` / `backoff_max` are also kept short so the
    // permanently-rejected SSE reconnect attempts do not stall the task
    // between polls, but the property under test is exercised purely through
    // the poll path: the SSE stream never opens even once in this test.
    let config = FlapsProviderConfig {
        base_url: format!("http://{}", handle.addr),
        sdk_key: SDK_SECRET.to_owned(),
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(5),
        snapshot_path: None,
        staleness_threshold: None,
        poll_interval: Duration::from_millis(50),
        backoff_base: Duration::from_millis(20),
        backoff_max: Duration::from_millis(60),
    };

    let mut provider = FlapsProvider::new(config);
    let ctx = EvaluationContext::default();
    timeout(Duration::from_secs(10), provider.initialize(&ctx))
        .await
        .expect("initialize timed out");

    // `initialize()` only spawns the background supervisor; it does not wait
    // for the first `fetch_and_store` to land. Wait for that first sync
    // (the one that runs BEFORE the SSE connect attempt, so it is unaffected
    // by the exhausted quota) before asserting on the initial value.
    let before = wait_for_resolved_value(&provider, &ctx, Duration::from_secs(2)).await;
    assert!(before, "seeded flag must serve true initially");

    let token = admin_login(handle.addr, ADMIN_PASSWORD).await;
    let body = serde_json::json!({
        "enabled": true,
        "rules": [],
        "default_rule": {"fixed": "off"},
    });
    put_flag_env_config(handle.addr, &token, PROJECT, FLAG, ENVIRONMENT, &body).await;

    // Poll every 20ms for up to 3 seconds - generous relative to the 50ms
    // poll_interval - until the client observes the new value. If the
    // supervisor never polls while SSE is unreachable, this loop times out
    // and the flag still resolves to `true`.
    let budget = Duration::from_secs(3);
    let start = Instant::now();
    loop {
        let current = provider
            .resolve_bool_value(FLAG, &ctx)
            .await
            .expect("flag must keep resolving while stale");
        if !current.value {
            break;
        }
        assert!(
            start.elapsed() < budget,
            "flag still serves the initial value after {budget:?}; the polling \
             fallback did not run while the SSE subscription was permanently \
             quota-rejected"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let after = provider
        .resolve_bool_value(FLAG, &ctx)
        .await
        .expect("flag must still resolve after the toggle");
    assert!(
        !after.value,
        "quota-rejected client must pick up the ruleset change via polling"
    );
    assert_eq!(after.variant, Some("off".to_owned()));
}
