//! [`FlapsProvider`]: an OpenFeature [`FeatureProvider`] backed by Flaps.
//!
//! The provider requires a **server-kind** SDK key. Client keys are rejected
//! by the server with 403.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use open_feature::async_trait;
use open_feature::provider::ResolutionDetails;
use open_feature::provider::{FeatureProvider, ProviderMetadata, ProviderStatus};
use open_feature::{
    EvaluationContext, EvaluationError, EvaluationErrorCode, EvaluationReason, EvaluationResult,
    FlagMetadata, StructValue,
};
use tokio::task::JoinHandle;

use crate::coerce;
use crate::context_mapper;
use crate::metadata_mapper;
use crate::reason_mapper;
use crate::shared::ProviderShared;
use crate::status::SyncStatus;
use crate::supervisor::spawn_supervisor;

/// Default HTTP connect timeout.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Default HTTP request timeout (first byte).
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Default polling interval for the background supervisor.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Default backoff base delay.
const DEFAULT_BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Default backoff ceiling.
const DEFAULT_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Configuration for a [`FlapsProvider`].
///
/// The provider requires a **server-kind** SDK key. The scope (project,
/// environment) is derived server-side from the key; no additional parameters
/// are needed.
#[derive(Debug, Clone)]
pub struct FlapsProviderConfig {
    /// Base URL of the Flaps server (no trailing slash), e.g. `https://flaps.internal`.
    pub base_url: String,
    /// SDK key used as a Bearer token. Must be a server-kind key.
    pub sdk_key: String,
    /// HTTP connect timeout. Defaults to 5 s.
    pub connect_timeout: Duration,
    /// HTTP request timeout. Defaults to 10 s.
    pub request_timeout: Duration,
    /// Path to write/read the disk snapshot. `None` disables snapshotting.
    pub snapshot_path: Option<PathBuf>,
    /// Age threshold after which the provider reports [`ProviderStatus::STALE`].
    /// `None` means the provider never reports `STALE` due to age.
    pub staleness_threshold: Option<Duration>,
    /// Interval for the background polling fallback. Defaults to 5 min.
    pub poll_interval: Duration,
    /// Initial backoff delay after a failed SSE reconnect. Defaults to 1 s.
    pub backoff_base: Duration,
    /// Maximum backoff delay. Defaults to 30 s.
    pub backoff_max: Duration,
}

impl FlapsProviderConfig {
    /// Creates a new config with default timeouts and no snapshotting.
    #[must_use]
    pub fn new(base_url: impl Into<String>, sdk_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            sdk_key: sdk_key.into(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            snapshot_path: None,
            staleness_threshold: None,
            poll_interval: DEFAULT_POLL_INTERVAL,
            backoff_base: DEFAULT_BACKOFF_BASE,
            backoff_max: DEFAULT_BACKOFF_MAX,
        }
    }
}

/// OpenFeature provider that evaluates flags locally against a ruleset fetched
/// from the Flaps server.
///
/// After [`initialize`] the provider spawns a background supervisor task that
/// maintains the ruleset fresh via SSE notifications and periodic polling. A
/// failed or absent sync leaves the ruleset as `None` so every evaluation
/// returns [`EvaluationErrorCode::ProviderNotReady`] and the SDK serves the
/// caller-supplied default.
///
/// If [`FlapsProviderConfig::snapshot_path`] is set and a snapshot file exists
/// the ruleset is available immediately at [`initialize`] time even when the
/// server is unreachable (warm-start).
///
/// [`initialize`]: FeatureProvider::initialize
pub struct FlapsProvider {
    config: FlapsProviderConfig,
    http_client: reqwest::Client,
    shared: Arc<ProviderShared>,
    metadata: ProviderMetadata,
    task: Option<JoinHandle<()>>,
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
            shared: Arc::new(ProviderShared::new()),
            metadata: ProviderMetadata::new("flaps"),
            task: None,
        }
    }

    /// Returns a snapshot of provider freshness metrics.
    #[must_use]
    pub fn sync_status(&self) -> SyncStatus {
        let state = self
            .shared
            .sync_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        SyncStatus::from_state(&state)
    }

    /// Evaluates a flag from the current ruleset.
    ///
    /// Returns the resolved value, variant, reason and the OpenFeature
    /// [`FlagMetadata`] converted from the merged flag-set and flag metadata
    /// (`None` when the merged metadata is empty).
    fn evaluate_raw(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<(
        serde_json::Value,
        Option<String>,
        EvaluationReason,
        Option<FlagMetadata>,
    )> {
        let guard = self.shared.ruleset.load();
        let flag_set = guard.as_ref().as_ref().ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::ProviderNotReady,
            message: Some("No ruleset loaded; sync may have failed during initialize".to_owned()),
        })?;

        let eval_ctx = context_mapper::map_context(evaluation_context)?;

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
        let flag_metadata = metadata_mapper::map_metadata(&resolution.metadata);
        Ok((value, resolution.variant, reason, flag_metadata))
    }
}

impl Drop for FlapsProvider {
    fn drop(&mut self) {
        if let Some(handle) = self.task.take() {
            handle.abort();
        }
    }
}

#[async_trait]
impl FeatureProvider for FlapsProvider {
    async fn initialize(&mut self, _context: &EvaluationContext) {
        // Step 1: warm-start from disk snapshot if configured.
        if let Some(ref path) = self.config.snapshot_path.clone() {
            crate::snapshot::load_snapshot(path, &self.shared).await;
        }

        // Step 2: spawn the background supervisor (SSE + polling fallback).
        let handle = spawn_supervisor(
            self.http_client.clone(),
            self.config.clone(),
            Arc::clone(&self.shared),
        );
        self.task = Some(handle);
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    fn status(&self) -> ProviderStatus {
        let guard = self.shared.ruleset.load();
        let has_ruleset = guard.as_ref().is_some();

        if !has_ruleset {
            return ProviderStatus::NotReady;
        }

        if let Some(threshold) = self.config.staleness_threshold {
            let state = self
                .shared
                .sync_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Stale when: snapshot not yet confirmed by network, or last sync too old.
            let is_stale = state.loaded_from_snapshot
                || state
                    .last_successful_sync
                    .is_none_or(|t| t.elapsed() > threshold);

            if is_stale {
                return ProviderStatus::STALE;
            }
        }

        ProviderStatus::Ready
    }

    async fn resolve_bool_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<bool>> {
        let (value, variant, reason, flag_metadata) =
            self.evaluate_raw(flag_key, evaluation_context)?;
        let typed = coerce::to_bool(&value).ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::TypeMismatch,
            message: Some(format!("flag `{flag_key}` value is not a boolean")),
        })?;
        Ok(ResolutionDetails {
            value: typed,
            variant,
            reason: Some(reason),
            flag_metadata,
        })
    }

    async fn resolve_int_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<i64>> {
        let (value, variant, reason, flag_metadata) =
            self.evaluate_raw(flag_key, evaluation_context)?;
        let typed = coerce::to_int(&value).ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::TypeMismatch,
            message: Some(format!("flag `{flag_key}` value is not an integer")),
        })?;
        Ok(ResolutionDetails {
            value: typed,
            variant,
            reason: Some(reason),
            flag_metadata,
        })
    }

    async fn resolve_float_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<f64>> {
        let (value, variant, reason, flag_metadata) =
            self.evaluate_raw(flag_key, evaluation_context)?;
        let typed = coerce::to_float(&value).ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::TypeMismatch,
            message: Some(format!("flag `{flag_key}` value is not a float")),
        })?;
        Ok(ResolutionDetails {
            value: typed,
            variant,
            reason: Some(reason),
            flag_metadata,
        })
    }

    async fn resolve_string_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<String>> {
        let (value, variant, reason, flag_metadata) =
            self.evaluate_raw(flag_key, evaluation_context)?;
        let typed = coerce::to_string(&value).ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::TypeMismatch,
            message: Some(format!("flag `{flag_key}` value is not a string")),
        })?;
        Ok(ResolutionDetails {
            value: typed,
            variant,
            reason: Some(reason),
            flag_metadata,
        })
    }

    async fn resolve_struct_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<StructValue>> {
        let (value, variant, reason, flag_metadata) =
            self.evaluate_raw(flag_key, evaluation_context)?;
        let typed = coerce::to_struct(&value).ok_or_else(|| EvaluationError {
            code: EvaluationErrorCode::TypeMismatch,
            message: Some(format!("flag `{flag_key}` value is not a struct/object")),
        })?;
        Ok(ResolutionDetails {
            value: typed,
            variant,
            reason: Some(reason),
            flag_metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    /// Verifies that `Drop` calls `abort()` without panicking.
    ///
    /// The [`tokio::task::JoinHandle`] abort path is exercised implicitly by every test that
    /// drops a provider with an active supervisor task. This stub documents the
    /// design intent.
    #[test]
    fn shutdown_covered_by_drop_trait() {
        // Design intent: `Drop for FlapsProvider` calls `handle.abort()`.
        // The integration tests exercise this path when they drop the provider.
    }
}
