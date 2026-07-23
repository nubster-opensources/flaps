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

        // Refuse what cannot be a key before spending a query on it.
        reject_impossible_sdk_key(&raw_key).map_err(|error| (StatusCode::UNAUTHORIZED, error))?;

        // Budget the attempt on the material actually presented.
        let client = ClientAddress::from_request_parts(parts, state)
            .await
            .unwrap_or(ClientAddress::Unknown);
        state
            .preauth_budget
            .consume(client, &raw_key)
            .map_err(|rejection| (StatusCode::TOO_MANY_REQUESTS, ApiError::from(rejection)))?;

        let record = state
            .store
            .find_sdk_key(&raw_key)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ApiError::Internal(e.to_string()),
                )
            })?
            .ok_or((StatusCode::UNAUTHORIZED, ApiError::Unauthorized))?;

        Ok(SdkKeyPrincipal {
            scope: record.scope,
            kind: record.kind,
            prefix: record.prefix,
        })
    }
}
