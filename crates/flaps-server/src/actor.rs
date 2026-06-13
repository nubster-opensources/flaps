//! Audit actor extraction from HTTP headers.

use axum::http::HeaderMap;

use crate::error::ApiError;

/// Extracts the audit actor from the `X-Flaps-Actor` header.
///
/// Required for every mutation (audit). Missing or empty header returns 422.
pub fn extract_actor(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get("X-Flaps-Actor")
        .ok_or_else(|| ApiError::InvalidBody("Missing required header X-Flaps-Actor".to_owned()))?;

    let actor = value
        .to_str()
        .map_err(|_| ApiError::InvalidBody("Header X-Flaps-Actor is not valid UTF-8".to_owned()))?
        .trim()
        .to_owned();

    if actor.is_empty() {
        return Err(ApiError::InvalidBody(
            "Header X-Flaps-Actor must not be empty".to_owned(),
        ));
    }

    Ok(actor)
}
