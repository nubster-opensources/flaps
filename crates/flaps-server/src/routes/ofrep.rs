//! OFREP v1 evaluation endpoints.
//!
//! Implements the [OFREP 0.3.0](https://github.com/open-feature/protocol) evaluation protocol:
//! - `POST /ofrep/v1/evaluate/flags/{key}` - single flag evaluation
//! - `POST /ofrep/v1/evaluate/flags` - bulk flag evaluation with ETag/304 support
//!
//! Both endpoints are authenticated via an SDK key (`Authorization: Bearer <key>`).
//! Rate limiting is applied per SDK key prefix. The hot path reads solely from the
//! in-memory compiled ruleset cache; the database is never queried.
//!
//! ## Atomicity guarantee
//!
//! The cache is a `HashMap` wrapped in a `RwLock`. Each evaluation acquires a
//! read guard, clones the required fields (`document`, `content_hash`, `version`),
//! then releases the guard before parsing. `install_in_cache` acquires a write guard
//! and replaces the entire entry atomically. A request therefore observes either the
//! previous complete ruleset or the new complete ruleset; it can never see a partial
//! update.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use flaps_eval::{EvaluationContext, EvaluationError, FlagSet, Reason};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    auth::SdkKeyPrincipal,
    error::ApiError,
    state::{AppState, Store},
};

// ---------------------------------------------------------------------------
// Request / response DTOs (OFREP 0.3.0 protocol boundary)
// ---------------------------------------------------------------------------

/// Evaluation request body shared by single and bulk endpoints.
#[derive(Debug, Deserialize)]
pub struct EvaluationRequest {
    /// Evaluation context carrying targeting key and arbitrary attributes.
    pub context: Option<ContextDto>,
}

/// OFREP evaluation context.
#[derive(Debug, Deserialize)]
pub struct ContextDto {
    /// Subject identifier exposed as `targetingKey` in targeting rules.
    #[serde(rename = "targetingKey")]
    pub targeting_key: Option<String>,
    /// Arbitrary extra attributes, flattened into the targeting scope.
    #[serde(flatten)]
    pub attributes: BTreeMap<String, Value>,
}

/// OFREP reason strings as defined by the 0.3.0 specification.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OfrRepReason {
    /// The flag has no targeting rule; the default variant was served.
    Static,
    /// The targeting rule selected a variant.
    TargetingMatch,
    /// The targeting rule returned `null`; the default variant was served.
    Default,
    /// The flag is disabled; the provider serves its own code default.
    Disabled,
}

/// OFREP error codes.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OfrRepErrorCode {
    /// The requested flag key is not present in the flag set.
    FlagNotFound,
    /// The evaluation context could not be parsed or is structurally invalid.
    InvalidContext,
    /// An internal error occurred on the server side.
    General,
}

/// Successful single flag evaluation response (OFREP serverEvaluationSuccess).
#[derive(Debug, Serialize)]
pub struct SingleSuccessResponse {
    /// The flag key that was evaluated.
    pub key: String,
    /// The resolved value, omitted when the flag is disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// The resolution reason.
    pub reason: OfrRepReason,
    /// The resolved variant key, omitted when no variant was resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Flag-set and flag metadata merged (flag entries win on collision),
    /// omitted entirely when empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, Value>>,
}

/// Converts a [`Resolution`]'s metadata to the OFREP DTO field: `None` when
/// empty, `Some` otherwise. Reuses `flaps_eval::metadata_to_json` as the
/// single source of truth for the JSON conversion.
fn metadata_field(metadata: &flaps_eval::Metadata) -> Option<serde_json::Map<String, Value>> {
    if metadata.is_empty() {
        return None;
    }
    match flaps_eval::metadata_to_json(metadata) {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// Failed single flag evaluation response (OFREP evaluationFailure / flagNotFound).
#[derive(Debug, Serialize)]
pub struct SingleErrorResponse {
    /// The flag key that was evaluated.
    pub key: String,
    /// OFREP error code.
    #[serde(rename = "errorCode")]
    pub error_code: OfrRepErrorCode,
    /// Human-readable error description.
    #[serde(rename = "errorDetails")]
    pub error_details: String,
}

/// One entry in the bulk flags array: either a success or an error.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum BulkFlagEntry {
    /// Successfully evaluated flag.
    Success(SingleSuccessResponse),
    /// Flag that could not be evaluated.
    Error(SingleErrorResponse),
}

/// Bulk evaluation metadata.
#[derive(Debug, Serialize)]
pub struct BulkMetadata {
    /// Monotone ruleset version, as a string per OFREP convention.
    pub version: String,
}

/// Bulk evaluation success response (OFREP bulkEvaluationSuccess).
#[derive(Debug, Serialize)]
pub struct BulkSuccessResponse {
    /// All evaluated flags (successes and per-flag errors coexist).
    pub flags: Vec<BulkFlagEntry>,
    /// Ruleset metadata.
    pub metadata: BulkMetadata,
}

/// Bulk or single evaluation failure response (context parse error).
#[derive(Debug, Serialize)]
pub struct EvaluationFailureResponse {
    /// OFREP error code.
    #[serde(rename = "errorCode")]
    pub error_code: OfrRepErrorCode,
    /// Human-readable error description.
    #[serde(rename = "errorDetails")]
    pub error_details: String,
}

// ---------------------------------------------------------------------------
// Internal mapping helpers
// ---------------------------------------------------------------------------

/// Maps a [`Reason`] to its OFREP string counterpart.
pub(crate) fn map_reason(reason: Reason) -> OfrRepReason {
    match reason {
        Reason::Static => OfrRepReason::Static,
        Reason::TargetingMatch => OfrRepReason::TargetingMatch,
        Reason::Default => OfrRepReason::Default,
        Reason::Disabled => OfrRepReason::Disabled,
    }
}

/// Extracts an [`EvaluationContext`] from the request DTO.
fn build_context(dto: Option<ContextDto>) -> EvaluationContext {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match dto {
        None => EvaluationContext {
            targeting_key: None,
            attributes: BTreeMap::new(),
            timestamp: now,
        },
        Some(ctx) => {
            let mut attributes = ctx.attributes;
            // `targetingKey` is extracted to the dedicated field; remove it
            // from attributes to avoid duplication in the scope.
            attributes.remove("targetingKey");
            EvaluationContext {
                targeting_key: ctx.targeting_key,
                attributes,
                timestamp: now,
            }
        }
    }
}

/// Maps an [`EvaluationError`] on a single flag to an HTTP status + OFREP error body.
fn single_eval_error_response(key: &str, err: EvaluationError) -> Response {
    match err {
        EvaluationError::FlagNotFound { flag_key } => {
            let body = SingleErrorResponse {
                key: key.to_owned(),
                error_code: OfrRepErrorCode::FlagNotFound,
                error_details: format!("flag `{flag_key}` not found"),
            };
            (StatusCode::NOT_FOUND, Json(body)).into_response()
        }
        EvaluationError::InvalidVariant { .. } | EvaluationError::UnsupportedOperation { .. } => {
            let body = SingleErrorResponse {
                key: key.to_owned(),
                error_code: OfrRepErrorCode::General,
                error_details: err.to_string(),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        }
    }
}

/// Formats a strong ETag value from a content hash.
///
/// Aligns with the format used in [`crate::etag`]: the hash is wrapped in double
/// quotes as required by RFC 7232.
fn format_etag(content_hash: &str) -> String {
    format!("\"{content_hash}\"")
}

/// Checks the `If-None-Match` header against the current ETag.
///
/// Returns `true` when the client ETag matches and a 304 should be served.
fn is_not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|client| client.trim() == etag)
}

/// Builds a 401 Unauthorized response in OFREP format.
fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(EvaluationFailureResponse {
            error_code: OfrRepErrorCode::General,
            error_details: "Missing or invalid SDK key.".to_owned(),
        }),
    )
        .into_response()
}

/// Builds a 429 Too Many Requests response with `Retry-After` header.
fn rate_limited_response(retry_after: u64) -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(EvaluationFailureResponse {
            error_code: OfrRepErrorCode::General,
            error_details: "Rate limit exceeded.".to_owned(),
        }),
    )
        .into_response();
    if let Ok(v) = HeaderValue::from_str(&retry_after.to_string()) {
        response.headers_mut().insert("Retry-After", v);
    }
    response
}

/// Evaluates all flags in a [`FlagSet`] against `ctx` and returns the bulk entries.
fn evaluate_all_flags(flag_set: &FlagSet, ctx: &EvaluationContext) -> Vec<BulkFlagEntry> {
    flag_set
        .flags
        .keys()
        .map(|flag_key| match flag_set.evaluate(flag_key, ctx) {
            Ok(resolution) => BulkFlagEntry::Success(SingleSuccessResponse {
                key: flag_key.clone(),
                value: resolution.value,
                reason: map_reason(resolution.reason),
                variant: resolution.variant,
                metadata: metadata_field(&resolution.metadata),
            }),
            Err(EvaluationError::FlagNotFound { flag_key: fk }) => {
                BulkFlagEntry::Error(SingleErrorResponse {
                    key: flag_key.clone(),
                    error_code: OfrRepErrorCode::FlagNotFound,
                    error_details: format!("flag `{fk}` not found"),
                })
            }
            Err(err) => BulkFlagEntry::Error(SingleErrorResponse {
                key: flag_key.clone(),
                error_code: OfrRepErrorCode::General,
                error_details: err.to_string(),
            }),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /ofrep/v1/evaluate/flags/{key}` - evaluate a single feature flag.
///
/// Authenticated via SDK key (server or client kind). Rate-limited per key prefix.
/// Reads from the in-memory cache; the database is never queried on this path.
///
/// ## OFREP 0.3.0 status codes
/// - 200 serverEvaluationSuccess
/// - 400 evaluationFailure (`INVALID_CONTEXT`)
/// - 401 unauthorized
/// - 404 flagNotFound (`FLAG_NOT_FOUND`)
/// - 429 too many requests (Retry-After header)
/// - 500 `GENERAL` (internal evaluation error)
pub async fn post_evaluate_flag<S: Store>(
    State(state): State<AppState<S>>,
    principal: Result<SdkKeyPrincipal, (StatusCode, ApiError)>,
    Path(key): Path<String>,
    body: Result<Json<EvaluationRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    // 1. Authenticate.
    let Ok(principal) = principal else {
        return unauthorized_response();
    };

    // 2. Rate limit.
    if let Err(retry_after) = state.rate_limiter.check(&principal.prefix) {
        return rate_limited_response(retry_after);
    }

    // 3. Parse request body.
    let Ok(Json(request)) = body else {
        return (
            StatusCode::BAD_REQUEST,
            Json(SingleErrorResponse {
                key: key.clone(),
                error_code: OfrRepErrorCode::InvalidContext,
                error_details: "Request body is not valid JSON.".to_owned(),
            }),
        )
            .into_response();
    };

    // 4. Lookup cache. Clone required fields and release the guard.
    let project_key = principal.scope.project_key.clone();
    let env_key = principal.scope.environment_key.clone();

    let entry = {
        let cache = state.cache.read().await;
        cache
            .get(&(project_key.clone(), env_key.clone()))
            .map(|r| (r.document.clone(), r.content_hash.clone(), r.version))
    };

    let Some((document, _, _)) = entry else {
        return (
            StatusCode::NOT_FOUND,
            Json(SingleErrorResponse {
                key: key.clone(),
                error_code: OfrRepErrorCode::FlagNotFound,
                error_details: format!("flag `{key}` not found"),
            }),
        )
            .into_response();
    };

    // 5. Parse flag set (hot path, per-request).
    let Ok(flag_set) = FlagSet::from_json(&document) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SingleErrorResponse {
                key: key.clone(),
                error_code: OfrRepErrorCode::General,
                error_details: "Failed to parse compiled ruleset.".to_owned(),
            }),
        )
            .into_response();
    };

    // 6. Build evaluation context.
    let ctx = build_context(request.context);

    // 7. Evaluate.
    match flag_set.evaluate(&key, &ctx) {
        Ok(resolution) => {
            let body = SingleSuccessResponse {
                key: key.clone(),
                value: resolution.value,
                reason: map_reason(resolution.reason),
                variant: resolution.variant,
                metadata: metadata_field(&resolution.metadata),
            };
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(err) => single_eval_error_response(&key, err),
    }
}

/// `POST /ofrep/v1/evaluate/flags` - bulk evaluate all flags.
///
/// Authenticated via SDK key (server or client kind). Rate-limited per key prefix.
/// Supports `If-None-Match` / 304 short-circuit based on the ruleset `content_hash`.
/// The `ETag` response header is always set on 200.
///
/// ## OFREP 0.3.0 status codes
/// - 200 bulkEvaluationSuccess (ETag header)
/// - 304 Not Modified (no body)
/// - 400 bulkEvaluationFailure (`INVALID_CONTEXT`)
/// - 401 unauthorized
/// - 429 too many requests (Retry-After header)
/// - 500 `GENERAL` (internal evaluation error)
pub async fn post_evaluate_flags<S: Store>(
    State(state): State<AppState<S>>,
    principal: Result<SdkKeyPrincipal, (StatusCode, ApiError)>,
    headers: HeaderMap,
    body: Result<Json<EvaluationRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    // 1. Authenticate.
    let Ok(principal) = principal else {
        return unauthorized_response();
    };

    // 2. Rate limit.
    if let Err(retry_after) = state.rate_limiter.check(&principal.prefix) {
        return rate_limited_response(retry_after);
    }

    // 3. Parse request body.
    let Ok(Json(request)) = body else {
        return (
            StatusCode::BAD_REQUEST,
            Json(EvaluationFailureResponse {
                error_code: OfrRepErrorCode::InvalidContext,
                error_details: "Request body is not valid JSON.".to_owned(),
            }),
        )
            .into_response();
    };

    // 4. Lookup cache. Clone required fields and release the guard.
    let project_key = principal.scope.project_key.clone();
    let env_key = principal.scope.environment_key.clone();

    let entry = {
        let cache = state.cache.read().await;
        cache
            .get(&(project_key.clone(), env_key.clone()))
            .map(|r| (r.document.clone(), r.content_hash.clone(), r.version))
    };

    // 5. Missing cache entry: return empty flags array.
    let Some((document, content_hash, version)) = entry else {
        let body = BulkSuccessResponse {
            flags: vec![],
            metadata: BulkMetadata {
                version: "0".to_owned(),
            },
        };
        return (StatusCode::OK, Json(body)).into_response();
    };

    // 6. ETag / 304 short-circuit.
    let etag = format_etag(&content_hash);
    if is_not_modified(&headers, &etag) {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    // 7. Parse flag set.
    let Ok(flag_set) = FlagSet::from_json(&document) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(EvaluationFailureResponse {
                error_code: OfrRepErrorCode::General,
                error_details: "Failed to parse compiled ruleset.".to_owned(),
            }),
        )
            .into_response();
    };

    // 8. Build evaluation context and evaluate all flags.
    let ctx = build_context(request.context);
    let flags = evaluate_all_flags(&flag_set, &ctx);

    // 9. Build response with ETag header.
    let response_body = BulkSuccessResponse {
        flags,
        metadata: BulkMetadata {
            version: version.to_string(),
        },
    };

    let mut response = (StatusCode::OK, Json(response_body)).into_response();
    if let Ok(v) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, v);
    }
    response
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use flaps_eval::Reason;

    // -------------------------------------------------------------------------
    // Reason mapping
    // -------------------------------------------------------------------------

    #[test]
    fn reason_static_maps_to_ofrep_static() {
        assert_eq!(map_reason(Reason::Static), OfrRepReason::Static);
    }

    #[test]
    fn reason_targeting_match_maps_to_ofrep_targeting_match() {
        assert_eq!(
            map_reason(Reason::TargetingMatch),
            OfrRepReason::TargetingMatch
        );
    }

    #[test]
    fn reason_default_maps_to_ofrep_default() {
        assert_eq!(map_reason(Reason::Default), OfrRepReason::Default);
    }

    #[test]
    fn reason_disabled_maps_to_ofrep_disabled() {
        assert_eq!(map_reason(Reason::Disabled), OfrRepReason::Disabled);
    }

    // -------------------------------------------------------------------------
    // EvaluationContext construction
    // -------------------------------------------------------------------------

    #[test]
    fn build_context_extracts_targeting_key() {
        let dto = Some(ContextDto {
            targeting_key: Some("user-123".to_owned()),
            attributes: BTreeMap::new(),
        });
        let ctx = build_context(dto);
        assert_eq!(ctx.targeting_key.as_deref(), Some("user-123"));
    }

    #[test]
    fn build_context_moves_extra_attributes() {
        let mut attrs = BTreeMap::new();
        attrs.insert("tier".to_owned(), Value::String("beta".to_owned()));
        let dto = Some(ContextDto {
            targeting_key: None,
            attributes: attrs,
        });
        let ctx = build_context(dto);
        assert_eq!(
            ctx.attributes.get("tier"),
            Some(&Value::String("beta".to_owned()))
        );
    }

    #[test]
    fn build_context_none_produces_empty_context() {
        let ctx = build_context(None);
        assert!(ctx.targeting_key.is_none());
        assert!(ctx.attributes.is_empty());
    }

    #[test]
    fn build_context_does_not_duplicate_targeting_key_in_attributes() {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "targetingKey".to_owned(),
            Value::String("should-be-removed".to_owned()),
        );
        attrs.insert("other".to_owned(), Value::String("value".to_owned()));
        let dto = Some(ContextDto {
            targeting_key: Some("user-abc".to_owned()),
            attributes: attrs,
        });
        let ctx = build_context(dto);
        assert!(!ctx.attributes.contains_key("targetingKey"));
        assert!(ctx.attributes.contains_key("other"));
    }

    // -------------------------------------------------------------------------
    // ETag helpers
    // -------------------------------------------------------------------------

    #[test]
    fn format_etag_wraps_hash_in_double_quotes() {
        let etag = format_etag("abc123");
        assert_eq!(etag, "\"abc123\"");
    }

    #[test]
    fn is_not_modified_returns_true_on_matching_etag() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"abc123\""),
        );
        assert!(is_not_modified(&headers, "\"abc123\""));
    }

    #[test]
    fn is_not_modified_returns_false_on_different_etag() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"old-hash\""),
        );
        assert!(!is_not_modified(&headers, "\"new-hash\""));
    }

    #[test]
    fn is_not_modified_returns_false_when_header_absent() {
        let headers = HeaderMap::new();
        assert!(!is_not_modified(&headers, "\"abc123\""));
    }

    // -------------------------------------------------------------------------
    // OfrRepReason serialization
    // -------------------------------------------------------------------------

    #[test]
    fn ofrep_reason_serializes_to_screaming_snake_case() {
        assert_eq!(
            serde_json::to_string(&OfrRepReason::Static).unwrap(),
            "\"STATIC\""
        );
        assert_eq!(
            serde_json::to_string(&OfrRepReason::TargetingMatch).unwrap(),
            "\"TARGETING_MATCH\""
        );
        assert_eq!(
            serde_json::to_string(&OfrRepReason::Default).unwrap(),
            "\"DEFAULT\""
        );
        assert_eq!(
            serde_json::to_string(&OfrRepReason::Disabled).unwrap(),
            "\"DISABLED\""
        );
    }
}
