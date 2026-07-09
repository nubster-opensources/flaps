//! End-to-end test (#26): kill switch propagation under two seconds.
//!
//! Exercises the real chain, no mocks :
//!
//! `PUT` admin config -> compile (`validate_by_compiling`) -> `install_in_cache`
//!   -> `SyncEvent` (broadcast) -> `SSE` `GET /sync/v1/events` -> refetch `GET /sync/v1/ruleset`
//!   -> `ArcSwap<FlagSet>` -> `resolve_bool_value` bascule.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use flaps_client::{FlapsProvider, FlapsProviderConfig};
use flaps_domain::{
    Environment, EnvironmentKey, Flag, FlagEnvConfig, FlagKey, FlagType, ManagedBy, Project,
    ProjectKey, SdkKeyKind, ServeTarget, ValueType, VariantKey, VariantValue, Variants,
};
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
use tokio::net::TcpListener;
use tokio::time::timeout;

const PROJECT: &str = "e2e-proj";
const ENVIRONMENT: &str = "e2e-env";
const FLAG: &str = "kill-switch-flag";
const ADMIN_PASSWORD: &str = "admin-pass";
const SDK_SECRET: &str = "s-e2e-kill-switch-server-0123456789";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A running Flaps server on a real ephemeral socket, seeded with a single
/// boolean flag active in one environment.
///
/// `state` is kept so the test can observe `state.events.receiver_count()`
/// (SSE subscriber readiness) directly, without guessing via sleeps.
struct ServerHandle {
    addr: SocketAddr,
    state: AppState<SqliteStore>,
}

/// Spawns a real Flaps server on an ephemeral port, seeded via the store
/// directly with one project, one environment, one boolean flag (enabled,
/// serving `on` = `true`) and one server SDK key.
async fn spawn_real_server() -> ServerHandle {
    let store = SqliteStore::in_memory(KeyHasher::new(b"e2e-kill-switch-pepper-32-bytes!"))
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

    let state = AppState::new(store);

    // Compiles the seeded environment and installs it into the cache, exactly
    // as `flapsd`'s startup warm-up does for every (project, environment)
    // pair. `warm_up_cache` itself lives in the `flapsd` binary crate, not in
    // `flaps-server`, so it is not reachable as a dev-dependency here; calling
    // `recompile_environment` directly for the single seeded environment is
    // the equivalent, narrower operation.
    flaps_server::recompile::recompile_environment(&state, &project_key, &env_key)
        .await
        .expect("initial compile");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral listener");
    let addr = listener.local_addr().expect("listener local addr");
    let app = flaps_server::build_router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("axum serve");
    });

    ServerHandle { addr, state }
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
                name: "e2e project".into(),
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
                name: "e2e environment".into(),
                external_ref: None,
                managed_by: ManagedBy::Local,
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
                name: "Kill switch flag".into(),
                description: None,
                flag_type: FlagType::Ops,
                value_type: ValueType::Boolean,
                variants,
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

/// Builds a real `FlapsProvider` pointed at `addr`, initializes it and returns
/// it once the first sync has completed.
///
/// `poll_interval` is set far beyond the test's lifetime so that any observed
/// propagation can only come from the SSE push path, never from the polling
/// fallback.
async fn start_synced_provider(addr: SocketAddr, sdk_key: &str) -> FlapsProvider {
    let config = FlapsProviderConfig {
        base_url: format!("http://{addr}"),
        sdk_key: sdk_key.to_owned(),
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(5),
        snapshot_path: None,
        staleness_threshold: None,
        poll_interval: Duration::from_secs(3600),
        backoff_base: Duration::from_millis(10),
        backoff_max: Duration::from_millis(50),
    };
    let mut provider = FlapsProvider::new(config);
    let ctx = EvaluationContext::default();
    timeout(Duration::from_secs(10), provider.initialize(&ctx))
        .await
        .expect("initialize timed out");
    provider
}

/// Waits until the provider's SSE subscription is live server-side, i.e.
/// `state.events.receiver_count() >= 1`.
///
/// A `broadcast::Sender` does not buffer for an absent subscriber: toggling
/// before this readiness point could lose the `SyncEvent` and force the test
/// to fall back on the (very long) poll interval, turning a real propagation
/// bug into a flaky timeout.
async fn wait_for_sse_subscriber(handle: &ServerHandle, budget: Duration) {
    let start = Instant::now();
    while handle.state.events.receiver_count() == 0 {
        assert!(
            start.elapsed() < budget,
            "no SSE subscriber within {budget:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Waits until the provider's reported ruleset version rises strictly above
/// `floor`, or panics once `budget` has elapsed.
///
/// Returns `(new_version, elapsed)`. The threshold is relative (`v > floor`)
/// rather than an absolute equality check: robust to whichever initial
/// version the compiler derivation happens to assign.
async fn wait_for_version_above(
    provider: &FlapsProvider,
    floor: u64,
    budget: Duration,
) -> (u64, Duration) {
    let start = Instant::now();
    loop {
        if let Some(v) = provider.sync_status().version {
            if v > floor {
                return (v, start.elapsed());
            }
        }
        assert!(
            start.elapsed() < budget,
            "version stayed <= {floor} within {budget:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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

/// Sends the real admin `PUT .../config` request that drives the whole chain
/// under test: `validate_by_compiling` -> `install_in_cache` -> `SyncEvent`.
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

#[tokio::test]
async fn harness_reaches_stable_initial_state() {
    let handle = spawn_real_server().await;
    let provider = start_synced_provider(handle.addr, SDK_SECRET).await;
    let ctx = EvaluationContext::default();

    wait_for_sse_subscriber(&handle, Duration::from_secs(2)).await;

    assert!(
        provider.sync_status().version.is_some(),
        "provider must know a ruleset version after the initial sync"
    );
    assert!(
        handle.state.events.receiver_count() >= 1,
        "server must see at least one live SSE subscriber"
    );

    let result = provider
        .resolve_bool_value(FLAG, &ctx)
        .await
        .expect("flag must resolve to Ok before any toggle");
    assert!(result.value, "seeded flag must serve true initially");
}

/// Sub-case A: the flag stays active, but the served value flips from `true`
/// (`on`) to `false` (`off`) via `default_rule`.
#[tokio::test]
async fn value_toggle_on_to_off_propagates_under_two_seconds() {
    let handle = spawn_real_server().await;
    let provider = start_synced_provider(handle.addr, SDK_SECRET).await;
    let ctx = EvaluationContext::default();

    wait_for_sse_subscriber(&handle, Duration::from_secs(2)).await;
    let v0 = provider
        .sync_status()
        .version
        .expect("version must be known after the initial sync");

    let before = provider
        .resolve_bool_value(FLAG, &ctx)
        .await
        .expect("flag must resolve before the toggle");
    assert!(before.value, "flag must serve true before the toggle");
    assert_eq!(before.variant, Some("on".to_owned()));

    // Obtain the admin token before starting the chrono: login is test
    // scaffolding (POST /login + argon2), not part of propagation.
    let token = admin_login(handle.addr, ADMIN_PASSWORD).await;
    let body = serde_json::json!({
        "enabled": true,
        "rules": [],
        "default_rule": {"fixed": "off"},
    });

    // Chrono starts here: readiness is complete (version known, SSE
    // subscriber live) and the token is in hand. Everything from this point
    // on is pure propagation: PUT config, then SSE-driven refetch client-side.
    let propagation_start = Instant::now();
    put_flag_env_config(handle.addr, &token, PROJECT, FLAG, ENVIRONMENT, &body).await;

    wait_for_version_above(&provider, v0, Duration::from_secs(2)).await;
    let elapsed = propagation_start.elapsed();

    let after = provider
        .resolve_bool_value(FLAG, &ctx)
        .await
        .expect("flag must still resolve after the toggle");
    assert!(!after.value, "flag must serve false after the toggle");
    assert_eq!(after.variant, Some("off".to_owned()));

    assert!(
        elapsed < Duration::from_secs(2),
        "propagation took {elapsed:?}, expected under two seconds"
    );
}

/// Sub-case B: a real kill switch. `enabled:false` propagates and resolution
/// flips from `Ok(true)` to `Err` (the documented DISABLED behaviour).
#[tokio::test]
async fn kill_switch_disables_flag_and_propagates_under_two_seconds() {
    let handle = spawn_real_server().await;
    let provider = start_synced_provider(handle.addr, SDK_SECRET).await;
    let ctx = EvaluationContext::default();

    wait_for_sse_subscriber(&handle, Duration::from_secs(2)).await;
    let v0 = provider
        .sync_status()
        .version
        .expect("version must be known after the initial sync");

    let before = provider
        .resolve_bool_value(FLAG, &ctx)
        .await
        .expect("flag must resolve before the kill switch");
    assert!(before.value, "flag must serve true before the kill switch");

    // Obtain the admin token before starting the chrono: login is test
    // scaffolding (POST /login + argon2), not part of propagation.
    let token = admin_login(handle.addr, ADMIN_PASSWORD).await;
    let body = serde_json::json!({
        "enabled": false,
        "rules": [],
        "default_rule": {"fixed": "on"},
    });

    // Chrono starts here: readiness is complete and the token is in hand.
    // Everything from this point on is pure propagation.
    let propagation_start = Instant::now();
    put_flag_env_config(handle.addr, &token, PROJECT, FLAG, ENVIRONMENT, &body).await;

    wait_for_version_above(&provider, v0, Duration::from_secs(2)).await;
    let elapsed = propagation_start.elapsed();

    let after = provider.resolve_bool_value(FLAG, &ctx).await;
    assert!(
        after.is_err(),
        "disabled flag must resolve to Err, got {after:?}"
    );

    assert!(
        elapsed < Duration::from_secs(2),
        "propagation took {elapsed:?}, expected under two seconds"
    );
}
