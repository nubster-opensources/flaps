//! Authentication extractors for admin sessions and SDK keys.

use axum::{
    extract::FromRequestParts,
    http::{StatusCode, header, request::Parts},
};
use flaps_domain::SdkKeyKind;
use flaps_store::SdkKeyScope;

use crate::{
    error::ApiError,
    preauth::{client_address::ClientAddress, sdk_key_shape::reject_impossible_sdk_key},
    state::{AppState, Store},
};

/// Authenticated admin principal extracted from a bearer session token.
///
/// Replaces the former `X-Flaps-Actor` header: the `username` field is used
/// as the audit actor for all mutations.
#[derive(Debug, Clone)]
pub struct AdminPrincipal {
    /// Account identifier.
    pub account_id: String,
    /// Human-readable login name, used as the audit actor.
    pub username: String,
}

/// Authenticated SDK client extracted from a bearer SDK key.
#[derive(Debug, Clone)]
pub struct SdkKeyPrincipal {
    /// The project/environment scope the key is bound to.
    pub scope: SdkKeyScope,
    /// Server or client SDK kind.
    pub kind: SdkKeyKind,
    /// Readable prefix of the key (used for rate limiting).
    pub prefix: String,
}

/// Extracts a bearer token from the `Authorization` header.
fn extract_bearer(parts: &Parts) -> Option<String> {
    let value = parts.headers.get(header::AUTHORIZATION)?;
    let s = value.to_str().ok()?;
    let token = s.strip_prefix("Bearer ")?;
    Some(token.to_owned())
}

impl<S: Store> FromRequestParts<AppState<S>> for AdminPrincipal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState<S>,
    ) -> Result<Self, Self::Rejection> {
        let raw_token = extract_bearer(parts).ok_or(ApiError::Unauthorized)?;

        let account = state
            .store
            .resolve_session(&raw_token)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .ok_or(ApiError::Unauthorized)?;

        Ok(AdminPrincipal {
            account_id: account.id,
            username: account.username,
        })
    }
}

impl<S: Store> FromRequestParts<AppState<S>> for SdkKeyPrincipal {
    type Rejection = (StatusCode, ApiError);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState<S>,
    ) -> Result<Self, Self::Rejection> {
        let raw_key =
            extract_bearer(parts).ok_or((StatusCode::UNAUTHORIZED, ApiError::Unauthorized))?;

        // Mal-formed keys are refused for free, before any budget or lookup.
        reject_impossible_sdk_key(&raw_key).map_err(|e| (StatusCode::UNAUTHORIZED, e))?;

        // Short-circuit before the lookup if this client has already spent its
        // failure budget. Peek only, so a valid key here never consumes anything.
        let client = ClientAddress::from_request_parts(parts, state)
            .await
            .unwrap_or(ClientAddress::Unknown); // Infallible
        state
            .preauth_budget
            .sdk_admits(client)
            .map_err(|r| (StatusCode::TOO_MANY_REQUESTS, ApiError::from(r)))?;

        let record = state.store.find_sdk_key(&raw_key).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::Internal(e.to_string()),
            )
        })?;

        if let Some(record) = record {
            Ok(SdkKeyPrincipal {
                scope: record.scope,
                kind: record.kind,
                prefix: record.prefix,
            })
        } else {
            // Only a FAILED lookup spends the budget: this is what bounds a
            // flood of well-formed but absent keys without ever touching
            // valid traffic.
            let _ = state.preauth_budget.consume_sdk_failure(client);
            Err((StatusCode::UNAUTHORIZED, ApiError::Unauthorized))
        }
    }
}
