//! Integration tests for OpenFeature `ResolutionDetails.flag_metadata`
//! (issue #109): `flaps-eval` merged flag-set and flag metadata must reach
//! every resolver method through the public OpenFeature client API.
//!
//! Covers:
//! - all five resolver methods (bool, string, int, float, struct) populate
//!   `flag_metadata`,
//! - flag-level entries win over flag-set-level entries on key collision,
//! - flag-set-level metadata still reaches a flag with no metadata of its
//!   own,
//! - integer-compatible and floating-point numeric metadata keep their type,
//! - empty metadata (no flag-set and no flag entries) maps to `None`.

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use open_feature::provider::{FeatureProvider, ProviderStatus};
use open_feature::{EvaluationContext, FlagMetadataValue};
use tokio::net::TcpListener;
use tokio::time::timeout;

use flaps_client::{FlapsProvider, FlapsProviderConfig};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A ruleset with flag-set level metadata (`environment`, `priority`) and,
/// on `bool-flag`, flag-level metadata that overrides `priority` and adds
/// entries of every supported scalar type. `no-metadata-flag` carries no
/// metadata of its own, to exercise flag-set-only inheritance.
const METADATA_DOCUMENT: &str = r#"
{
  "flags": {
    "bool-flag": {
      "state": "ENABLED",
      "variants": { "on": true, "off": false },
      "defaultVariant": "on",
      "metadata": {
        "owner": "team-flags",
        "priority": 2,
        "rollout": 0.5,
        "enabled": true
      }
    },
    "string-flag": {
      "state": "ENABLED",
      "variants": { "a": "hello", "b": "world" },
      "defaultVariant": "a",
      "metadata": { "owner": "team-strings" }
    },
    "int-flag": {
      "state": "ENABLED",
      "variants": { "low": 1, "high": 100 },
      "defaultVariant": "low",
      "metadata": { "owner": "team-ints" }
    },
    "float-flag": {
      "state": "ENABLED",
      "variants": { "half": 1.5 },
      "defaultVariant": "half",
      "metadata": { "owner": "team-floats" }
    },
    "struct-flag": {
      "state": "ENABLED",
      "variants": { "cfg": { "retries": 3 } },
      "defaultVariant": "cfg",
      "metadata": { "owner": "team-structs" }
    },
    "no-metadata-flag": {
      "state": "ENABLED",
      "variants": { "on": true, "off": false },
      "defaultVariant": "on"
    }
  },
  "metadata": {
    "environment": "test",
    "priority": 1
  }
}
"#;

/// A ruleset carrying no metadata anywhere (neither flag-set nor flag level).
const NO_METADATA_DOCUMENT: &str = r#"
{
  "flags": {
    "plain-flag": {
      "state": "ENABLED",
      "variants": { "on": true, "off": false },
      "defaultVariant": "on"
    }
  }
}
"#;

// ---------------------------------------------------------------------------
// Test server + provider helpers
// ---------------------------------------------------------------------------

/// Spawns a mock server serving a fixed flagd `document` for `/sync/v1/ruleset`.
async fn spawn_document_server(document: &'static str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let app = Router::new().route(
        "/sync/v1/ruleset",
        get(move || async move { document_response(document) }),
    );

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    addr
}

fn document_response(document: &'static str) -> Response {
    let mut response = axum::response::Response::new(axum::body::Body::from(document));
    let h = response.headers_mut();
    h.insert("X-Flaps-Version", "1".parse().unwrap());
    h.insert(
        axum::http::header::ETAG,
        "\"etag-metadata\"".parse().unwrap(),
    );
    response.into_response()
}

/// Builds a minimal config pointed at `addr` with fast timeouts and no
/// background interference.
fn fast_config(addr: SocketAddr) -> FlapsProviderConfig {
    FlapsProviderConfig {
        base_url: format!("http://{addr}"),
        sdk_key: "test-key".to_owned(),
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(5),
        snapshot_path: None,
        staleness_threshold: None,
        poll_interval: Duration::from_secs(3600),
        backoff_base: Duration::from_millis(10),
        backoff_max: Duration::from_millis(50),
    }
}

/// Builds a provider, calls `initialize`, and returns it once the first sync
/// has completed.
///
/// Polls `status()` rather than sleeping a fixed duration: `initialize` only
/// spawns the background supervisor, it does not wait for the first fetch to
/// complete, and a fixed sleep is flaky under CPU contention (e.g. other
/// crates compiling concurrently in the same `cargo test` run).
async fn synced_provider(addr: SocketAddr) -> FlapsProvider {
    let mut provider = FlapsProvider::new(fast_config(addr));
    let ctx = EvaluationContext::default();
    timeout(Duration::from_secs(10), provider.initialize(&ctx))
        .await
        .expect("initialize timed out");

    let start = std::time::Instant::now();
    while provider.status() != ProviderStatus::Ready {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "provider did not become Ready within 5s"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    provider
}

// ---------------------------------------------------------------------------
// AC3: all five resolvers populate `flag_metadata`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bool_resolution_includes_merged_metadata() {
    let addr = spawn_document_server(METADATA_DOCUMENT).await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();

    let result = provider
        .resolve_bool_value("bool-flag", &ctx)
        .await
        .expect("bool-flag must resolve");
    let metadata = result
        .flag_metadata
        .expect("bool-flag metadata must be present");

    assert_eq!(
        metadata.values.get("owner"),
        Some(&FlagMetadataValue::String("team-flags".to_owned()))
    );
    assert_eq!(
        metadata.values.get("rollout"),
        Some(&FlagMetadataValue::Float(0.5))
    );
    assert_eq!(
        metadata.values.get("enabled"),
        Some(&FlagMetadataValue::Bool(true))
    );
    assert_eq!(
        metadata.values.get("environment"),
        Some(&FlagMetadataValue::String("test".to_owned())),
        "flag-set level metadata must also be present when not overridden"
    );
}

#[tokio::test]
async fn string_resolution_includes_metadata() {
    let addr = spawn_document_server(METADATA_DOCUMENT).await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();

    let result = provider
        .resolve_string_value("string-flag", &ctx)
        .await
        .expect("string-flag must resolve");
    let metadata = result
        .flag_metadata
        .expect("string-flag metadata must be present");

    assert_eq!(
        metadata.values.get("owner"),
        Some(&FlagMetadataValue::String("team-strings".to_owned()))
    );
}

#[tokio::test]
async fn int_resolution_includes_metadata() {
    let addr = spawn_document_server(METADATA_DOCUMENT).await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();

    let result = provider
        .resolve_int_value("int-flag", &ctx)
        .await
        .expect("int-flag must resolve");
    let metadata = result
        .flag_metadata
        .expect("int-flag metadata must be present");

    assert_eq!(
        metadata.values.get("owner"),
        Some(&FlagMetadataValue::String("team-ints".to_owned()))
    );
}

#[tokio::test]
async fn float_resolution_includes_metadata() {
    let addr = spawn_document_server(METADATA_DOCUMENT).await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();

    let result = provider
        .resolve_float_value("float-flag", &ctx)
        .await
        .expect("float-flag must resolve");
    let metadata = result
        .flag_metadata
        .expect("float-flag metadata must be present");

    assert_eq!(
        metadata.values.get("owner"),
        Some(&FlagMetadataValue::String("team-floats".to_owned()))
    );
}

#[tokio::test]
async fn struct_resolution_includes_metadata() {
    let addr = spawn_document_server(METADATA_DOCUMENT).await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();

    let result = provider
        .resolve_struct_value("struct-flag", &ctx)
        .await
        .expect("struct-flag must resolve");
    let metadata = result
        .flag_metadata
        .expect("struct-flag metadata must be present");

    assert_eq!(
        metadata.values.get("owner"),
        Some(&FlagMetadataValue::String("team-structs".to_owned()))
    );
}

// ---------------------------------------------------------------------------
// Flag-over-environment precedence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flag_level_metadata_wins_over_flag_set_level_on_collision() {
    let addr = spawn_document_server(METADATA_DOCUMENT).await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();

    let result = provider
        .resolve_bool_value("bool-flag", &ctx)
        .await
        .expect("bool-flag must resolve");
    let metadata = result.flag_metadata.expect("metadata must be present");

    // The flag-set declares `priority: 1`; `bool-flag` overrides it to `2`.
    assert_eq!(
        metadata.values.get("priority"),
        Some(&FlagMetadataValue::Int(2)),
        "flag-level metadata must win over flag-set-level metadata"
    );
}

#[tokio::test]
async fn flag_set_level_metadata_reaches_flag_without_own_metadata() {
    let addr = spawn_document_server(METADATA_DOCUMENT).await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();

    let result = provider
        .resolve_bool_value("no-metadata-flag", &ctx)
        .await
        .expect("no-metadata-flag must resolve");
    let metadata = result
        .flag_metadata
        .expect("flag-set metadata must reach a flag with no metadata of its own");

    assert_eq!(
        metadata.values.get("environment"),
        Some(&FlagMetadataValue::String("test".to_owned()))
    );
    assert_eq!(
        metadata.values.get("priority"),
        Some(&FlagMetadataValue::Int(1))
    );
}

// ---------------------------------------------------------------------------
// AC4: empty metadata maps to the absent OpenFeature representation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_metadata_maps_to_none() {
    let addr = spawn_document_server(NO_METADATA_DOCUMENT).await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();

    let result = provider
        .resolve_bool_value("plain-flag", &ctx)
        .await
        .expect("plain-flag must resolve");
    assert!(
        result.flag_metadata.is_none(),
        "metadata absent everywhere must map to None, not Some(empty)"
    );
}
