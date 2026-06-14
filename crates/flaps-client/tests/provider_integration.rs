//! Integration tests for [`FlapsProvider`].
//!
//! Tests that exercise the full evaluation path: provider without a loaded
//! ruleset must return errors without panicking, and after a successful sync
//! evaluations must return the correct typed values with the right reason.

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::response::Response;
use axum::routing::get;
use open_feature::EvaluationContext;
use open_feature::provider::FeatureProvider;
use tokio::net::TcpListener;
use tokio::time::timeout;

use flaps_client::{FlapsProvider, FlapsProviderConfig};

// Minimal flagd document used in sync tests.
const FLAGD_DOCUMENT: &str = r#"
{
  "flags": {
    "bool-flag": {
      "state": "ENABLED",
      "variants": { "on": true, "off": false },
      "defaultVariant": "on"
    },
    "string-flag": {
      "state": "ENABLED",
      "variants": { "a": "hello", "b": "world" },
      "defaultVariant": "a"
    },
    "int-flag": {
      "state": "ENABLED",
      "variants": { "low": 1, "high": 100 },
      "defaultVariant": "low"
    },
    "float-flag": {
      "state": "ENABLED",
      "variants": { "half": 1.5 },
      "defaultVariant": "half"
    },
    "struct-flag": {
      "state": "ENABLED",
      "variants": { "cfg": { "retries": 3 } },
      "defaultVariant": "cfg"
    },
    "disabled-flag": {
      "state": "DISABLED",
      "variants": { "on": true, "off": false },
      "defaultVariant": "on"
    }
  }
}
"#;

/// Spawn a minimal axum server that serves the flagd document and returns
/// `X-Flaps-Version: 42`.
async fn spawn_mock_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let app = Router::new().route("/sync/v1/ruleset", get(ruleset_handler));

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    addr
}

async fn ruleset_handler() -> Response {
    let mut response = axum::response::Response::new(axum::body::Body::from(FLAGD_DOCUMENT));
    response
        .headers_mut()
        .insert("X-Flaps-Version", "42".parse().unwrap());
    response
}

/// Build a provider pointed at the given address, call initialize and return it.
async fn synced_provider(addr: SocketAddr) -> FlapsProvider {
    let config = FlapsProviderConfig {
        base_url: format!("http://{addr}"),
        sdk_key: "test-key".to_owned(),
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(5),
    };
    let mut provider = FlapsProvider::new(config);
    let ctx = EvaluationContext::default();
    timeout(Duration::from_secs(5), provider.initialize(&ctx))
        .await
        .expect("initialize timed out");
    provider
}

// ---------------------------------------------------------------------------
// Tests: provider without a loaded ruleset
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_ruleset_bool_returns_err_without_panic() {
    let config = FlapsProviderConfig::new("http://127.0.0.1:1", "bad-key");
    let provider = FlapsProvider::new(config);
    let ctx = EvaluationContext::default();
    let result = provider.resolve_bool_value("any-flag", &ctx).await;
    assert!(result.is_err(), "expected Err when ruleset is absent");
}

#[tokio::test]
async fn no_ruleset_string_returns_err_without_panic() {
    let config = FlapsProviderConfig::new("http://127.0.0.1:1", "bad-key");
    let provider = FlapsProvider::new(config);
    let ctx = EvaluationContext::default();
    let result = provider.resolve_string_value("any-flag", &ctx).await;
    assert!(result.is_err(), "expected Err when ruleset is absent");
}

#[tokio::test]
async fn no_ruleset_int_returns_err_without_panic() {
    let config = FlapsProviderConfig::new("http://127.0.0.1:1", "bad-key");
    let provider = FlapsProvider::new(config);
    let ctx = EvaluationContext::default();
    let result = provider.resolve_int_value("any-flag", &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn no_ruleset_float_returns_err_without_panic() {
    let config = FlapsProviderConfig::new("http://127.0.0.1:1", "bad-key");
    let provider = FlapsProvider::new(config);
    let ctx = EvaluationContext::default();
    let result = provider.resolve_float_value("any-flag", &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn no_ruleset_struct_returns_err_without_panic() {
    let config = FlapsProviderConfig::new("http://127.0.0.1:1", "bad-key");
    let provider = FlapsProvider::new(config);
    let ctx = EvaluationContext::default();
    let result = provider.resolve_struct_value("any-flag", &ctx).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Tests: sync against a real (in-process) HTTP server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_loads_bool_flag() {
    let addr = spawn_mock_server().await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();
    let result = provider
        .resolve_bool_value("bool-flag", &ctx)
        .await
        .expect("should resolve bool flag after sync");
    assert!(result.value);
    assert_eq!(result.variant, Some("on".to_owned()));
}

#[tokio::test]
async fn sync_loads_string_flag() {
    let addr = spawn_mock_server().await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();
    let result = provider
        .resolve_string_value("string-flag", &ctx)
        .await
        .expect("should resolve string flag after sync");
    assert_eq!(result.value, "hello");
}

#[tokio::test]
async fn sync_loads_int_flag() {
    let addr = spawn_mock_server().await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();
    let result = provider
        .resolve_int_value("int-flag", &ctx)
        .await
        .expect("should resolve int flag after sync");
    assert_eq!(result.value, 1);
}

#[tokio::test]
async fn sync_loads_float_flag() {
    let addr = spawn_mock_server().await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();
    let result = provider
        .resolve_float_value("float-flag", &ctx)
        .await
        .expect("should resolve float flag after sync");
    assert!((result.value - 1.5_f64).abs() < f64::EPSILON);
}

#[tokio::test]
async fn sync_loads_struct_flag() {
    let addr = spawn_mock_server().await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();
    let result = provider
        .resolve_struct_value("struct-flag", &ctx)
        .await
        .expect("should resolve struct flag after sync");
    assert!(result.value.fields.contains_key("retries"));
}

// ---------------------------------------------------------------------------
// Tests: type mismatch never panics, always returns Err
// ---------------------------------------------------------------------------

#[tokio::test]
async fn type_mismatch_string_as_bool_returns_err() {
    let addr = spawn_mock_server().await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();
    // "string-flag" has string variants; asking for bool must fail
    let result = provider.resolve_bool_value("string-flag", &ctx).await;
    assert!(
        result.is_err(),
        "type mismatch should return Err, not panic"
    );
    let err = result.unwrap_err();
    assert_eq!(err.code, open_feature::EvaluationErrorCode::TypeMismatch);
}

#[tokio::test]
async fn type_mismatch_bool_as_string_returns_err() {
    let addr = spawn_mock_server().await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();
    // "bool-flag" has bool variants; asking for string must fail
    let result = provider.resolve_string_value("bool-flag", &ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, open_feature::EvaluationErrorCode::TypeMismatch);
}

// ---------------------------------------------------------------------------
// Tests: disabled flag returns Err (not a panic)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disabled_flag_returns_err_without_panic() {
    let addr = spawn_mock_server().await;
    let provider = synced_provider(addr).await;
    let ctx = EvaluationContext::default();
    let result = provider.resolve_bool_value("disabled-flag", &ctx).await;
    assert!(
        result.is_err(),
        "disabled flag should return Err, not panic"
    );
}

// ---------------------------------------------------------------------------
// Tests: status() exposes version and last_successful_sync after sync
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_has_version_after_sync() {
    let addr = spawn_mock_server().await;
    let provider = synced_provider(addr).await;
    let status = provider.status();
    assert_eq!(
        status.version,
        Some(42),
        "version should be read from X-Flaps-Version header"
    );
    assert!(
        status.last_successful_sync.is_some(),
        "last_successful_sync should be set after sync"
    );
    assert!(
        status.ruleset_age.is_some(),
        "ruleset_age should be Some after sync"
    );
}

#[tokio::test]
async fn status_is_none_before_sync() {
    let config = FlapsProviderConfig::new("http://127.0.0.1:1", "bad-key");
    let provider = FlapsProvider::new(config);
    let status = provider.status();
    assert!(status.version.is_none());
    assert!(status.last_successful_sync.is_none());
    assert!(status.ruleset_age.is_none());
}
