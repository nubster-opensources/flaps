//! Oracle test for issue #109.
//!
//! `flaps-server`'s OFREP endpoint already exposes the merged flag-set and
//! flag metadata with flag-over-environment precedence (see
//! `flaps_server::routes::ofrep::metadata_field`). This test runs the real
//! chain, no mocks: seed via `flaps-store` -> compile via `flaps-compiler`
//! -> serve via `flaps-server`, then asserts that the in-process
//! `FlapsProvider`'s `ResolutionDetails.flag_metadata` (local path) carries
//! the exact same entries, with the same types, as the OFREP HTTP response
//! (remote path) for the same flag.

use std::net::SocketAddr;
use std::time::Duration;

use flaps_client::{FlapsProvider, FlapsProviderConfig};
use flaps_domain::{
    Environment, EnvironmentKey, Flag, FlagEnvConfig, FlagKey, FlagType, ManagedBy,
    Metadata as DomainMetadata, MetadataValue as DomainMetadataValue, Project, ProjectKey,
    SdkKeyKind, ServeTarget, ValueType, VariantKey, VariantValue, Variants,
};
use flaps_server::state::AppState;
use flaps_store::hash::KeyHasher;
use flaps_store::repository::{
    EnvironmentRepository as _, FlagEnvConfigRepository as _, FlagRepository as _,
    ProjectRepository as _, SdkKeyRepository as _,
};
use flaps_store::sdk_key::{NewSdkKey, SdkKeyScope};
use flaps_store::sqlite::SqliteStore;
use open_feature::provider::{FeatureProvider, ProviderStatus};
use open_feature::{EvaluationContext, FlagMetadataValue};
use tokio::net::TcpListener;
use tokio::time::timeout;

const PROJECT: &str = "metadata-oracle-proj";
const ENVIRONMENT: &str = "metadata-oracle-env";
const FLAG: &str = "metadata-oracle-flag";
const SDK_SECRET: &str = "s-metadata-oracle-server-0123456789";

/// Seeds `store` with one project, one environment carrying its own
/// metadata, one boolean flag whose metadata overrides one environment-level
/// entry and adds entries of every supported scalar type, and one server SDK
/// key scoped to that project and environment.
async fn seed_flag_with_metadata(store: &SqliteStore) {
    let project_key = ProjectKey::new(PROJECT).expect("valid project key");
    let env_key = EnvironmentKey::new(ENVIRONMENT).expect("valid environment key");
    let flag_key = FlagKey::new(FLAG).expect("valid flag key");
    let vk_on = VariantKey::new("on").expect("valid variant key");
    let vk_off = VariantKey::new("off").expect("valid variant key");

    store
        .upsert_project(
            "system",
            &Project {
                key: project_key.clone(),
                name: "metadata oracle project".into(),
                description: None,
                external_ref: None,
                managed_by: ManagedBy::Local,
            },
        )
        .await
        .expect("upsert project");

    let mut environment_metadata = DomainMetadata::new();
    environment_metadata.insert(
        "environment".to_owned(),
        DomainMetadataValue::String("test".into()),
    );
    // Overridden by the flag-level `priority` below: exercises
    // flag-over-environment precedence through the real merge path.
    environment_metadata.insert("priority".to_owned(), DomainMetadataValue::Number(1.0));

    store
        .upsert_environment(
            "system",
            &project_key,
            &Environment {
                key: env_key.clone(),
                name: "metadata oracle environment".into(),
                external_ref: None,
                managed_by: ManagedBy::Local,
                metadata: environment_metadata,
            },
        )
        .await
        .expect("upsert environment");

    let variants = Variants::new(
        ValueType::Boolean,
        [
            (vk_on.clone(), VariantValue::Bool(true)),
            (vk_off.clone(), VariantValue::Bool(false)),
        ],
    )
    .expect("valid variants");

    let mut flag_metadata = DomainMetadata::new();
    flag_metadata.insert(
        "owner".to_owned(),
        DomainMetadataValue::String("team-flags".into()),
    );
    flag_metadata.insert("priority".to_owned(), DomainMetadataValue::Number(2.0));
    flag_metadata.insert("rollout".to_owned(), DomainMetadataValue::Number(0.5));
    flag_metadata.insert("enabled".to_owned(), DomainMetadataValue::Bool(true));

    store
        .upsert_flag(
            "system",
            &project_key,
            &Flag {
                key: flag_key.clone(),
                name: "Metadata oracle flag".into(),
                description: None,
                flag_type: FlagType::Release,
                value_type: ValueType::Boolean,
                variants,
                metadata: flag_metadata,
            },
        )
        .await
        .expect("upsert flag");

    store
        .upsert_flag_env_config(
            "system",
            &project_key,
            &flag_key,
            &env_key,
            &FlagEnvConfig {
                enabled: true,
                rules: vec![],
                default_rule: ServeTarget::Fixed(vk_on.clone()),
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

/// A real Flaps server on an ephemeral port, seeded via
/// [`seed_flag_with_metadata`] and compiled through the real
/// `flaps-compiler` path.
async fn spawn_server_with_metadata() -> SocketAddr {
    let store = SqliteStore::in_memory(KeyHasher::new(b"metadata-oracle-pepper-32-bytes!"))
        .await
        .expect("in-memory store");
    seed_flag_with_metadata(&store).await;

    let project_key = ProjectKey::new(PROJECT).expect("valid project key");
    let env_key = EnvironmentKey::new(ENVIRONMENT).expect("valid environment key");

    let state = AppState::new(store);
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

    addr
}

/// Fetches the OFREP metadata object for `FLAG` directly over HTTP: the
/// oracle this test derives its expectations from.
async fn fetch_ofrep_metadata(addr: SocketAddr) -> serde_json::Map<String, serde_json::Value> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/ofrep/v1/evaluate/flags/{FLAG}"))
        .bearer_auth(SDK_SECRET)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("ofrep request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "OFREP evaluation must succeed"
    );
    let body: serde_json::Value = resp.json().await.expect("ofrep response body");
    body.get("metadata")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// Asserts that a raw OFREP JSON metadata scalar and the corresponding local
/// `FlagMetadataValue` represent the same value AND the same type. For a
/// JSON number, the OFREP wire form itself is the type oracle: an
/// integer-form number (`Number::is_i64` or `Number::is_u64`, i.e. no
/// fractional part) must correspond to a local `FlagMetadataValue::Int`, and
/// a fractional-form number must correspond to a local `Float`. This is
/// deliberately independent of which local variant happens to be present,
/// so a genuine Int<->Float divergence between the OFREP mapper and the
/// local mapper cannot hide behind an accidental value match (e.g. OFREP
/// `2` vs local `Float(2.0)`).
fn assert_entry_matches(
    key: &str,
    json_value: &serde_json::Value,
    local_value: &FlagMetadataValue,
) {
    match (json_value, local_value) {
        (serde_json::Value::Bool(expected), FlagMetadataValue::Bool(actual)) => {
            assert_eq!(expected, actual, "key `{key}` bool value mismatch");
        }
        (serde_json::Value::String(expected), FlagMetadataValue::String(actual)) => {
            assert_eq!(expected, actual, "key `{key}` string value mismatch");
        }
        (serde_json::Value::Number(expected), _) => {
            let is_integer_form = expected.is_i64() || expected.is_u64();
            match (is_integer_form, local_value) {
                (true, FlagMetadataValue::Int(actual)) => {
                    assert_eq!(
                        expected.as_i64(),
                        Some(*actual),
                        "key `{key}` int value mismatch"
                    );
                }
                (false, FlagMetadataValue::Float(actual)) => {
                    let expected_f64 = expected
                        .as_f64()
                        .unwrap_or_else(|| panic!("key `{key}` OFREP number is not a valid f64"));
                    assert!(
                        (expected_f64 - actual).abs() < f64::EPSILON,
                        "key `{key}` float value mismatch: OFREP {expected_f64} vs local {actual}"
                    );
                }
                (true, FlagMetadataValue::Float(actual)) => panic!(
                    "key `{key}` int/float type divergence: OFREP carries an \
                     integer-form number ({expected}) but local metadata carries \
                     Float({actual})"
                ),
                (false, FlagMetadataValue::Int(actual)) => panic!(
                    "key `{key}` int/float type divergence: OFREP carries a \
                     fractional-form number ({expected}) but local metadata carries \
                     Int({actual})"
                ),
                (_, other) => panic!(
                    "key `{key}` type mismatch between OFREP number {expected} and local {other:?}"
                ),
            }
        }
        _ => panic!(
            "key `{key}` type mismatch between OFREP {json_value:?} and local {local_value:?}"
        ),
    }
}

/// Polls `provider.status()` until it reports `Ready`, or panics once
/// `budget` has elapsed. `initialize` only spawns the background supervisor;
/// it does not wait for the first fetch to complete.
async fn wait_until_ready(provider: &FlapsProvider, budget: Duration) {
    let start = std::time::Instant::now();
    while provider.status() != ProviderStatus::Ready {
        assert!(
            start.elapsed() < budget,
            "provider did not become Ready within {budget:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn local_resolution_metadata_matches_ofrep() {
    let addr = spawn_server_with_metadata().await;

    let config = FlapsProviderConfig {
        base_url: format!("http://{addr}"),
        sdk_key: SDK_SECRET.to_owned(),
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
    wait_until_ready(&provider, Duration::from_secs(5)).await;

    let local = provider
        .resolve_bool_value(FLAG, &ctx)
        .await
        .expect("local resolution must succeed");
    let local_metadata = local
        .flag_metadata
        .expect("flag has non-empty merged metadata");

    let oracle_metadata = fetch_ofrep_metadata(addr).await;
    assert!(
        !oracle_metadata.is_empty(),
        "OFREP oracle must itself carry metadata for this fixture"
    );

    assert_eq!(
        local_metadata.values.len(),
        oracle_metadata.len(),
        "local and OFREP metadata must carry the same number of entries"
    );
    for (key, json_value) in &oracle_metadata {
        let local_value = local_metadata
            .values
            .get(key)
            .unwrap_or_else(|| panic!("local metadata is missing key `{key}` present in OFREP"));
        assert_entry_matches(key, json_value, local_value);
    }
}
