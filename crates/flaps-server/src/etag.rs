//! ETag computation from canonical JSON serialization, and `If-Match` /
//! `If-None-Match` precondition checks (RFC 7232).
//!
//! All `ETag`s produced by [`compute_etag`] are **strong**: a hex-encoded
//! SHA-256 digest of the resource's canonical JSON representation. The API
//! never emits or accepts a weak `ETag` (`W/"..."`), and precondition checks
//! here always use the strong comparison function, so a weak-tagged
//! `If-Match` value is compared byte-for-byte like any other value and never
//! receives special treatment.

use axum::http::{HeaderMap, HeaderName};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::ApiError;

/// Reads a precondition header (`If-Match` or `If-None-Match`) from `headers`,
/// joining every field-line present into a single value.
///
/// [`HeaderMap::get`] returns only the FIRST field-line for a repeated header
/// name. [RFC 7230 §3.2.2](https://www.rfc-editor.org/rfc/rfc7230#section-3.2.2)
/// makes repeated field-lines semantically identical to one comma-joined
/// value, and [RFC 7232](https://www.rfc-editor.org/rfc/rfc7232) defines
/// `If-Match` / `If-None-Match` as exactly such lists. Reading only the first
/// line lets a second line silently override the first: for example
/// `If-None-Match: "x"` followed by `If-None-Match: *` would be seen as just
/// `"x"`, taking the non-`*` branch and bypassing the create-only guard. This
/// function reads with [`HeaderMap::get_all`] and joins every line with
/// `", "` before returning, so the caller sees the same value it would if the
/// client had sent one comma-separated line.
///
/// Returns `Ok(None)` when the header is absent (no precondition). Returns
/// `Err(ApiError::InvalidBody)` when any field-line is not valid ASCII: a
/// malformed precondition header must never be silently treated as absent
/// (`.to_str().ok()` collapsing to `None` would let a write proceed
/// unconditionally while the client believes it sent an active precondition).
pub fn read_precondition_header(
    headers: &HeaderMap,
    name: &HeaderName,
) -> Result<Option<String>, ApiError> {
    let mut lines = headers.get_all(name).iter().peekable();
    if lines.peek().is_none() {
        return Ok(None);
    }

    let mut parts = Vec::new();
    for line in lines {
        let value = line.to_str().map_err(|_| {
            ApiError::InvalidBody(format!("{name} header value is not valid ASCII"))
        })?;
        parts.push(value);
    }
    Ok(Some(parts.join(", ")))
}

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

/// Compares an optional `If-Match` header against the current ETag of a resource.
///
/// `current_etag` is `None` when the addressed resource does not currently
/// exist (e.g. it was never created, or it has just been deleted).
///
/// Follows [RFC 7232 §3.1](https://www.rfc-editor.org/rfc/rfc7232#section-3.1):
/// - Absent header: no precondition, always `Ok(())`.
/// - `*`: the condition is true only if a current representation exists.
/// - A comma-separated list of `ETag`s: true if any listed value equals the
///   current `ETag`. All comparisons here use the strong comparison function
///   (see the module-level docs): `ETag`s in this API are always strong, so
///   weak comparison (`W/"..."`) is not supported and a weak-tagged value
///   never matches.
///
/// Returns `Err(PreconditionFailed)` when the condition evaluates to false.
pub fn check_if_match(if_match: Option<&str>, current_etag: Option<&str>) -> Result<(), ApiError> {
    let Some(header_value) = if_match else {
        return Ok(());
    };
    let header_value = header_value.trim();

    if header_value == "*" {
        return if current_etag.is_some() {
            Ok(())
        } else {
            Err(ApiError::PreconditionFailed)
        };
    }

    let Some(current) = current_etag else {
        // A specific ETag list cannot match a resource that does not exist.
        return Err(ApiError::PreconditionFailed);
    };

    let matches = header_value
        .split(',')
        .map(|raw| raw.trim().trim_matches('"'))
        .any(|candidate| candidate == current);

    if matches {
        Ok(())
    } else {
        Err(ApiError::PreconditionFailed)
    }
}

/// Enforces the `If-None-Match: *` create-only guard.
///
/// Follows [RFC 7232 §3.2](https://www.rfc-editor.org/rfc/rfc7232#section-3.2)
/// for the "create-only" idiom on `PUT`: `If-None-Match: *` succeeds only when
/// no current representation of the resource exists, and fails with 412
/// otherwise. Absent header: no precondition, always `Ok(())`.
///
/// Only the `*` form is supported here. This is a deliberate scope choice,
/// not a gap in RFC coverage: RFC 7232 §3.2 explicitly defines the general
/// listed-ETags form of `If-None-Match` for methods other than `GET`/`HEAD`
/// too ("the origin server MUST NOT perform the requested method... unless...
/// the origin server MUST respond with... 412"), so a client sending it is
/// making a well-formed request. This API's write-side guard only ever needs
/// the "create, never overwrite" idiom, so the listed form is unimplemented.
/// Rather than silently ignore it (which would fail open: the client believes
/// it sent an active precondition and gets a 200/201 unconditionally), an
/// unsupported listed value is rejected with `422 Unprocessable Entity`.
pub fn check_if_none_match(if_none_match: Option<&str>, exists: bool) -> Result<(), ApiError> {
    let Some(header_value) = if_none_match else {
        return Ok(());
    };

    if header_value.trim() == "*" {
        if exists {
            Err(ApiError::PreconditionFailed)
        } else {
            Ok(())
        }
    } else {
        Err(ApiError::InvalidBody(
            "If-None-Match only supports the '*' create-only guard on this API; the listed-ETags \
             form is not implemented"
                .to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{check_if_match, check_if_none_match};
    use crate::error::ApiError;

    const CURRENT: &str = "abc123";

    fn assert_precondition_failed<T: std::fmt::Debug>(result: &Result<T, ApiError>) {
        assert!(
            matches!(result, Err(ApiError::PreconditionFailed)),
            "expected PreconditionFailed, got {result:?}"
        );
    }

    // -- check_if_match: absent header -----------------------------------

    #[test]
    fn if_match_absent_is_always_ok() {
        assert!(check_if_match(None, Some(CURRENT)).is_ok());
        assert!(check_if_match(None, None).is_ok());
    }

    // -- check_if_match: single ETag --------------------------------------

    #[test]
    fn if_match_single_matching_etag_is_ok() {
        assert!(check_if_match(Some(CURRENT), Some(CURRENT)).is_ok());
    }

    #[test]
    fn if_match_single_matching_quoted_etag_is_ok() {
        let quoted = format!("\"{CURRENT}\"");
        assert!(check_if_match(Some(&quoted), Some(CURRENT)).is_ok());
    }

    #[test]
    fn if_match_single_mismatching_etag_is_precondition_failed() {
        assert_precondition_failed(&check_if_match(Some("other-etag"), Some(CURRENT)));
    }

    // -- check_if_match: comma-separated list (RFC 7232 SS3.1) -------------

    #[test]
    fn if_match_list_matches_when_any_member_equals_current() {
        let list = format!("\"etag-a\", \"{CURRENT}\", \"etag-c\"");
        assert!(check_if_match(Some(&list), Some(CURRENT)).is_ok());
    }

    #[test]
    fn if_match_list_fails_when_no_member_matches() {
        let list = "\"etag-a\", \"etag-b\"";
        assert_precondition_failed(&check_if_match(Some(list), Some(CURRENT)));
    }

    // -- check_if_match: wildcard (RFC 7232 SS3.1) --------------------------

    #[test]
    fn if_match_star_is_ok_when_resource_exists() {
        assert!(check_if_match(Some("*"), Some(CURRENT)).is_ok());
    }

    #[test]
    fn if_match_star_is_precondition_failed_when_resource_missing() {
        assert_precondition_failed(&check_if_match(Some("*"), None));
    }

    // -- check_if_match: specific ETag against a missing resource ----------

    #[test]
    fn if_match_specific_etag_is_precondition_failed_when_resource_missing() {
        assert_precondition_failed(&check_if_match(Some(CURRENT), None));
    }

    // -- check_if_none_match: absent header ---------------------------------

    #[test]
    fn if_none_match_absent_is_always_ok() {
        assert!(check_if_none_match(None, true).is_ok());
        assert!(check_if_none_match(None, false).is_ok());
    }

    // -- check_if_none_match: wildcard create-only guard (RFC 7232 SS3.2) ----

    #[test]
    fn if_none_match_star_is_ok_when_resource_absent() {
        assert!(check_if_none_match(Some("*"), false).is_ok());
    }

    #[test]
    fn if_none_match_star_is_precondition_failed_when_resource_exists() {
        assert_precondition_failed(&check_if_none_match(Some("*"), true));
    }

    // -- check_if_none_match: unsupported listed-ETags form (Fix 5) ---------

    #[test]
    fn if_none_match_listed_form_is_rejected_as_invalid_body() {
        let result = check_if_none_match(Some(CURRENT), false);
        assert!(
            matches!(result, Err(ApiError::InvalidBody(_))),
            "expected InvalidBody, got {result:?}"
        );

        let result = check_if_none_match(Some(CURRENT), true);
        assert!(
            matches!(result, Err(ApiError::InvalidBody(_))),
            "expected InvalidBody, got {result:?}"
        );
    }

    // -- read_precondition_header: multi-line join (Fix 3) -------------------

    mod read_precondition_header_tests {
        use axum::http::{HeaderMap, HeaderValue, header};

        use super::super::read_precondition_header;
        use crate::error::ApiError;

        #[test]
        fn absent_header_is_none() {
            let headers = HeaderMap::new();
            let result = read_precondition_header(&headers, &header::IF_NONE_MATCH).unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn single_line_is_returned_as_is() {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_MATCH, HeaderValue::from_static("\"abc123\""));
            let result = read_precondition_header(&headers, &header::IF_MATCH).unwrap();
            assert_eq!(result.as_deref(), Some("\"abc123\""));
        }

        /// The dangerous case (Fix 3): two `If-None-Match` field-lines, the
        /// second of which is `*`. Reading only the first line (as
        /// `HeaderMap::get` does) would see just `"x"` and silently bypass the
        /// create-only guard. Joining every line must surface the `*`.
        #[test]
        fn two_lines_are_joined_with_comma_space() {
            let mut headers = HeaderMap::new();
            headers.append(header::IF_NONE_MATCH, HeaderValue::from_static("\"x\""));
            headers.append(header::IF_NONE_MATCH, HeaderValue::from_static("*"));

            let result = read_precondition_header(&headers, &header::IF_NONE_MATCH).unwrap();

            assert_eq!(result.as_deref(), Some("\"x\", *"));
        }

        /// A header present but not valid ASCII must fail closed (Fix 4),
        /// never collapse to `None` (which `check_if_match`/`check_if_none_match`
        /// treat as "no precondition").
        #[test]
        fn non_ascii_header_value_is_invalid_body() {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::IF_MATCH,
                HeaderValue::from_bytes(b"\xffnot-ascii").unwrap(),
            );

            let result = read_precondition_header(&headers, &header::IF_MATCH);

            assert!(
                matches!(result, Err(ApiError::InvalidBody(_))),
                "expected InvalidBody, got {result:?}"
            );
        }

        /// A malformed SECOND line must also fail closed, even when the first
        /// line is well-formed: the join must not silently drop the bad line.
        #[test]
        fn non_ascii_second_line_is_invalid_body() {
            let mut headers = HeaderMap::new();
            headers.append(header::IF_MATCH, HeaderValue::from_static("\"x\""));
            headers.append(
                header::IF_MATCH,
                HeaderValue::from_bytes(b"\xffnot-ascii").unwrap(),
            );

            let result = read_precondition_header(&headers, &header::IF_MATCH);

            assert!(
                matches!(result, Err(ApiError::InvalidBody(_))),
                "expected InvalidBody, got {result:?}"
            );
        }
    }
}
