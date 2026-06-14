//! Ruleset sync endpoints for server-side SDK consumers.
//!
//! Provides two routes (server-key only, 403 on client keys):
//!
//! - `GET /sync/v1/ruleset`: downloads the compiled flagd ruleset for the
//!   environment bound to the SDK key, with `ETag` / `304 Not Modified` support.
//! - `GET /sync/v1/events`: a server-sent events stream that delivers one
//!   [`EventPayload`] per recompilation. The payload carries only the environment
//!   key and the new version; it never exposes flag data.
//!
//! ## Ordering invariant
//!
//! Each event is emitted **after** the corresponding ruleset is written to the
//! cache (inside [`crate::recompile::install_in_cache`]). A subscriber that
//! receives an event and immediately calls `GET /sync/v1/ruleset` will always
//! observe a version that is equal to or greater than the one announced in the
//! event; it can never see a stale entry.

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use flaps_domain::{EnvironmentKey, ProjectKey, SdkKeyKind};
use serde::Serialize;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::{
    auth::SdkKeyPrincipal,
    error::ApiError,
    state::{AppState, Store},
};

// ---------------------------------------------------------------------------
// Bus event
// ---------------------------------------------------------------------------

/// An event broadcast on the internal channel after each cache update.
///
/// The `project` field is used for filtering only; it is never sent to clients.
#[derive(Debug, Clone)]
pub struct SyncEvent {
    /// The project this ruleset belongs to (used for scoped filtering).
    pub project: ProjectKey,
    /// The environment whose ruleset was recompiled.
    pub environment: EnvironmentKey,
    /// Monotone version of the newly installed ruleset.
    pub version: u64,
}

// ---------------------------------------------------------------------------
// SSE payload
// ---------------------------------------------------------------------------

/// Payload carried by each SSE frame on `GET /sync/v1/events`.
///
/// Intentionally minimal: only `environment` and `version` are exposed.
/// Flag data is never included.
#[derive(Debug, Serialize)]
pub struct EventPayload {
    /// The environment key whose ruleset changed.
    pub environment: String,
    /// Monotone version of the new ruleset.
    pub version: u64,
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Formats a strong ETag value (quoted content hash) for the `ETag` header.
fn format_etag(content_hash: &str) -> String {
    format!("\"{content_hash}\"")
}

/// Returns `true` when the client `If-None-Match` header matches `etag` exactly.
fn is_not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|client| client.trim() == etag)
}

/// Enforces that the principal holds a Server key; returns 403 otherwise.
fn require_server_key(kind: SdkKeyKind) -> Result<(), ApiError> {
    if kind == SdkKeyKind::Server {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

// ---------------------------------------------------------------------------
// Download handler
// ---------------------------------------------------------------------------

/// `GET /sync/v1/ruleset` - download the compiled flagd ruleset for the SDK key scope.
///
/// ## Authentication
/// Requires a server-kind SDK key. Client keys receive 403.
///
/// ## Caching
/// The `ETag` header carries the `content_hash` of the ruleset. Clients should
/// send `If-None-Match` on subsequent requests; the server returns 304 when the
/// ruleset is unchanged.
///
/// ## Extra headers
/// - `ETag`: strong ETag based on the content hash.
/// - `X-Flaps-Version`: monotone version counter of the ruleset.
///
/// ## Status codes
/// - 200: ruleset JSON body (`application/json`).
/// - 304: not modified (no body).
/// - 401: missing or invalid SDK key.
/// - 403: client-kind SDK key.
/// - 404: no compiled ruleset in cache for this scope.
/// - 429: rate limit exceeded (`Retry-After` header set).
pub async fn get_ruleset<S: Store>(
    State(state): State<AppState<S>>,
    principal: Result<SdkKeyPrincipal, (StatusCode, ApiError)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // 1. Authenticate.
    let principal = principal.map_err(|(_, e)| e)?;

    // 2. Server-key only.
    require_server_key(principal.kind)?;

    // 3. Rate limit.
    state
        .rate_limiter
        .check(&principal.prefix)
        .map_err(|retry_after_seconds| ApiError::TooManyRequests {
            retry_after_seconds,
        })?;

    // 4. Lookup cache entry.
    let project_key = principal.scope.project_key.clone();
    let env_key = principal.scope.environment_key.clone();

    let entry = {
        let cache = state.cache.read().await;
        cache
            .get(&(project_key, env_key))
            .map(|r| (r.document.clone(), r.content_hash.clone(), r.version))
    };

    let Some((document, content_hash, version)) = entry else {
        return Err(ApiError::NotFound);
    };

    // 5. ETag / 304 short-circuit.
    let etag = format_etag(&content_hash);
    if is_not_modified(&headers, &etag) {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }

    // 6. Build 200 response with ETag + X-Flaps-Version headers.
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        document,
    )
        .into_response();

    let response_headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&etag) {
        response_headers.insert(header::ETAG, v);
    }
    if let Ok(v) = HeaderValue::from_str(&version.to_string()) {
        response_headers.insert("X-Flaps-Version", v);
    }

    Ok(response)
}

// ---------------------------------------------------------------------------
// SSE handler
// ---------------------------------------------------------------------------

/// `GET /sync/v1/events` - SSE stream of ruleset change notifications.
///
/// ## Authentication
/// Requires a server-kind SDK key. Client keys receive 403.
///
/// ## Stream format
/// Each SSE frame carries a JSON-encoded [`EventPayload`] with `environment`
/// and `version`. No flag data is ever included.
///
/// ## Lag handling
/// If a subscriber cannot keep up with the broadcast buffer, lagged ticks are
/// skipped silently. The client should re-sync via `GET /sync/v1/ruleset` after
/// reconnecting.
///
/// ## Status codes
/// - 200: SSE stream opened successfully.
/// - 401: missing or invalid SDK key.
/// - 403: client-kind SDK key.
pub async fn get_events<S: Store>(
    State(state): State<AppState<S>>,
    principal: Result<SdkKeyPrincipal, (StatusCode, ApiError)>,
) -> Result<impl IntoResponse, ApiError> {
    // 1. Authenticate.
    let principal = principal.map_err(|(_, e)| e)?;

    // 2. Server-key only.
    require_server_key(principal.kind)?;

    // 3. Subscribe BEFORE releasing the principal (no async gap).
    let rx = state.events.subscribe();
    let scope_project = principal.scope.project_key.clone();
    let scope_env = principal.scope.environment_key.clone();

    // 4. Build the filtered event stream.
    let stream = BroadcastStream::new(rx).filter_map(move |result| {
        match result {
            // Lagged: skip, do not terminate the stream.
            Err(_) => None,
            Ok(ev) => {
                if ev.project != scope_project || ev.environment != scope_env {
                    return None;
                }
                let payload = EventPayload {
                    environment: ev.environment.as_str().to_owned(),
                    version: ev.version,
                };
                match Event::default().json_data(&payload) {
                    Ok(sse_event) => Some(Ok::<_, std::convert::Infallible>(sse_event)),
                    Err(_) => None,
                }
            }
        }
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_payload_serializes_environment_and_version_only() {
        let payload = EventPayload {
            environment: "production".to_owned(),
            version: 42,
        };
        let json = serde_json::to_value(&payload).expect("serialization must succeed");
        let obj = json.as_object().expect("must be a JSON object");

        assert_eq!(
            obj.get("environment").and_then(|v| v.as_str()),
            Some("production")
        );
        assert_eq!(
            obj.get("version").and_then(serde_json::Value::as_u64),
            Some(42)
        );
        assert_eq!(obj.len(), 2, "payload must have exactly 2 fields");
    }

    #[test]
    fn event_payload_has_no_flag_data_fields() {
        let payload = EventPayload {
            environment: "staging".to_owned(),
            version: 1,
        };
        let json = serde_json::to_value(&payload).expect("serialization must succeed");
        let obj = json.as_object().expect("must be a JSON object");

        assert!(
            !obj.contains_key("flags"),
            "payload must not expose flag data"
        );
        assert!(
            !obj.contains_key("document"),
            "payload must not expose document"
        );
        assert!(
            !obj.contains_key("project"),
            "payload must not expose project"
        );
    }

    #[test]
    fn format_etag_wraps_hash_in_double_quotes() {
        assert_eq!(format_etag("abc123"), "\"abc123\"");
    }

    #[test]
    fn is_not_modified_returns_true_on_exact_match() {
        use axum::http::header;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"hash42\""),
        );
        assert!(is_not_modified(&headers, "\"hash42\""));
    }

    #[test]
    fn is_not_modified_returns_false_on_mismatch() {
        use axum::http::header;
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"old\""));
        assert!(!is_not_modified(&headers, "\"new\""));
    }

    #[test]
    fn is_not_modified_returns_false_when_absent() {
        let headers = HeaderMap::new();
        assert!(!is_not_modified(&headers, "\"hash42\""));
    }

    #[test]
    fn require_server_key_accepts_server() {
        assert!(require_server_key(SdkKeyKind::Server).is_ok());
    }

    #[test]
    fn require_server_key_rejects_client() {
        let err = require_server_key(SdkKeyKind::Client).unwrap_err();
        assert!(matches!(err, ApiError::Forbidden));
    }
}
