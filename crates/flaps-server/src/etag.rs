//! ETag computation from canonical JSON serialization.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::ApiError;

/// Recursively sorts object keys in a `serde_json::Value` for stable serialization.
fn canonical_json(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: Vec<(String, Value)> = map
                .into_iter()
                .map(|(k, v)| (k, canonical_json(v)))
                .collect();
            sorted.sort_by(|(a, _), (b, _)| a.cmp(b));
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical_json).collect()),
        other => other,
    }
}

/// Returns the strong ETag of a resource: hex SHA-256 of its canonical JSON.
///
/// Serialization is canonical (stable key order) so the ETag is stable even
/// when the resource contains `HashMap`-backed fields like `Variants`.
pub fn compute_etag<T: Serialize>(value: &T) -> Result<String, ApiError> {
    let raw = serde_json::to_value(value)
        .map_err(|e| ApiError::Internal(format!("ETag serialization failed: {e}")))?;
    let canonical = canonical_json(raw);
    let canonical_str = serde_json::to_string(&canonical)
        .map_err(|e| ApiError::Internal(format!("ETag canonical serialization failed: {e}")))?;

    let mut hasher = Sha256::new();
    hasher.update(canonical_str.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

/// Compares an optional `If-Match` header against the current ETag.
///
/// Returns `Ok(())` when absent (no precondition) or matching.
/// Returns `Err(PreconditionFailed)` when present and different.
pub fn check_if_match(if_match: Option<&str>, current_etag: &str) -> Result<(), ApiError> {
    let Some(client_etag) = if_match else {
        return Ok(());
    };
    // Strip surrounding quotes if present (HTTP ETag format uses quoted strings).
    let client_etag = client_etag.trim_matches('"');
    if client_etag == current_etag {
        Ok(())
    } else {
        Err(ApiError::PreconditionFailed)
    }
}
