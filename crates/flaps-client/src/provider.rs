//! [`FlapsProvider`]: an OpenFeature [`FeatureProvider`] backed by Flaps.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;
use flaps_eval::FlagSet;
use open_feature::async_trait;
use open_feature::provider::ResolutionDetails;
use open_feature::provider::{FeatureProvider, ProviderMetadata};
use open_feature::{
    EvaluationContext, EvaluationError, EvaluationErrorCode, EvaluationReason, EvaluationResult,
    StructValue,
};

use crate::coerce;
use crate::context_mapper;
use crate::reason_mapper;
use crate::status::SyncState;
use crate::sync::fetch_and_store;

/// Default HTTP connect timeout.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Default HTTP request timeout (first byte).
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Configuration for a [`FlapsProvider`].
#[derive(Debug, Clone)]
pub struct FlapsProviderConfig {
    /// Base URL of the Flaps server (no trailing slash), e.g. `https://flaps.internal`.
    pub base_url: String,
    /// SDK key used as a Bearer token.
    pub sdk_key: String,
    /// HTTP connect timeout. Defaults to 5 s.
    pub connect_timeout: Duration,
    /// HTTP request timeout. Defaults to 10 s.
    pub request_timeout: Duration,
}

impl FlapsProviderConfig {
    /// Creates a new config with default timeouts.
    #[must_use]
    pub fn new(base_url: impl Into<String>, sdk_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            sdk_key: sdk_key.into(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

/// OpenFeature provider that evaluates flags locally against a ruleset fetched
/// from the Flaps server.
///
/// The ruleset is downloaded once during [`initialize`] and stored in an
/// [`ArcSwap`] for lock-free reads on the evaluation hot path. A failed or
/// absent sync leaves the ruleset as `None` so every evaluation returns
/// [`EvaluationErrorCode::ProviderNotReady`] and the SDK serves the
/// caller-supplied default.
///
/// [`initialize`]: FeatureProvider::initialize
pub struct FlapsProvider {
    config: FlapsProviderConfig,
    http_client: reqwest::Client,
    ruleset: ArcSwap<Option<Arc<FlagSet>>>,
    sync_state: Mutex<SyncState>,
    metadata: ProviderMetadata,
}

impl FlapsProvider {
    /// Creates a new provider from `config`.
    ///
    /// The ruleset is not fetched until the OpenFeature SDK calls
    /// [`initialize`].
    ///
    /// [`initialize`]: FeatureProvider::initialize
    #[must_use]
    pub fn new(config: FlapsProviderConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .build()
            .unwrap_or_default();

        Self {
            config,
            http_client,
            ruleset: ArcSwap::new(Arc::new(None)),
            sync_state: Mutex::new(SyncState::default()),
            metadata: ProviderMetadata::new("flaps"),
        }
    }

    /// Returns a snapshot of provider freshness metrics.
    #[must_use]
    pub fn status(&self) -> crate::SyncStatus {
        let state = self
            .sync_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::status::SyncStatus::from_state(&state)
    }

    /// Evaluates a flag from the current ruleset.
    ///
    /// Returns `Err` when the ruleset is absent, the flag cannot be found,
    /// or the resolved value does not match the expected type.
    fn evaluate_raw(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<(serde_json::Value, Option<String>, EvaluationReason)> {
        let guard = self.ruleset.load();
        let flag_set = guard.as_ref().as_ref().ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::ProviderNotReady,
            message: Some("No ruleset loaded; sync may have failed during initialize".to_owned()),
        })?;

        let eval_ctx = context_mapper::map_context(evaluation_context);

        let resolution = flag_set.evaluate(flag_key, &eval_ctx).map_err(|e| {
            use flaps_eval::EvaluationError as EvalErr;
            let (code, msg) = match e {
                EvalErr::FlagNotFound { flag_key: ref k } => (
                    EvaluationErrorCode::FlagNotFound,
                    format!("flag `{k}` not found"),
                ),
                EvalErr::InvalidVariant {
                    ref flag_key,
                    ref resolved,
                } => (
                    EvaluationErrorCode::General("INVALID_VARIANT".to_owned()),
                    format!("flag `{flag_key}` targeting resolved to invalid variant: {resolved}"),
                ),
                EvalErr::UnsupportedOperation { operator } => (
                    EvaluationErrorCode::General("UNSUPPORTED_OPERATION".to_owned()),
                    format!("unsupported operator `{operator}`"),
                ),
            };
            EvaluationError {
                code,
                message: Some(msg),
            }
        })?;

        let value = resolution.value.ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::General("DISABLED_OR_NO_VARIANT".to_owned()),
            message: Some(format!(
                "flag `{flag_key}` is disabled or has no variant; caller default applies"
            )),
        })?;

        let reason = reason_mapper::map_reason(resolution.reason);
        Ok((value, resolution.variant, reason))
    }
}

#[async_trait]
impl FeatureProvider for FlapsProvider {
    async fn initialize(&mut self, _context: &EvaluationContext) {
        fetch_and_store(
            &self.http_client,
            &self.config.base_url,
            &self.config.sdk_key,
            &self.ruleset,
            &self.sync_state,
        )
        .await;
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn resolve_bool_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<bool>> {
        let (value, variant, reason) = self.evaluate_raw(flag_key, evaluation_context)?;
        let typed = coerce::to_bool(&value).ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::TypeMismatch,
            message: Some(format!("flag `{flag_key}` value is not a boolean")),
        })?;
        Ok(ResolutionDetails {
            value: typed,
            variant,
            reason: Some(reason),
            flag_metadata: None,
        })
    }

    async fn resolve_int_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<i64>> {
        let (value, variant, reason) = self.evaluate_raw(flag_key, evaluation_context)?;
        let typed = coerce::to_int(&value).ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::TypeMismatch,
            message: Some(format!("flag `{flag_key}` value is not an integer")),
        })?;
        Ok(ResolutionDetails {
            value: typed,
            variant,
            reason: Some(reason),
            flag_metadata: None,
        })
    }

    async fn resolve_float_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<f64>> {
        let (value, variant, reason) = self.evaluate_raw(flag_key, evaluation_context)?;
        let typed = coerce::to_float(&value).ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::TypeMismatch,
            message: Some(format!("flag `{flag_key}` value is not a float")),
        })?;
        Ok(ResolutionDetails {
            value: typed,
            variant,
            reason: Some(reason),
            flag_metadata: None,
        })
    }

    async fn resolve_string_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<String>> {
        let (value, variant, reason) = self.evaluate_raw(flag_key, evaluation_context)?;
        let typed = coerce::to_string(&value).ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::TypeMismatch,
            message: Some(format!("flag `{flag_key}` value is not a string")),
        })?;
        Ok(ResolutionDetails {
            value: typed,
            variant,
            reason: Some(reason),
            flag_metadata: None,
        })
    }

    async fn resolve_struct_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<StructValue>> {
        let (value, variant, reason) = self.evaluate_raw(flag_key, evaluation_context)?;
        let typed = coerce::to_struct(&value).ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::TypeMismatch,
            message: Some(format!("flag `{flag_key}` value is not a struct/object")),
        })?;
        Ok(ResolutionDetails {
            value: typed,
            variant,
            reason: Some(reason),
            flag_metadata: None,
        })
    }
}
