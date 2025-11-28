//! Flaps SDK client.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use flaps_core::{
    EvaluationContext, EvaluationReason, EvaluationResult, Evaluator, Flag, FlagValue, Segment,
    SegmentId,
};

use crate::config::Config;

/// The Flaps SDK client for evaluating feature flags.
///
/// The client maintains a local cache of flags and evaluates them locally
/// for optimal performance. It syncs with the server via SSE or polling.
pub struct FlapsClient {
    config: Config,
    evaluator: Evaluator,
    flags: Arc<RwLock<HashMap<String, Flag>>>,
    #[allow(dead_code)]
    segments: Arc<RwLock<HashMap<String, Segment>>>,
}

impl FlapsClient {
    /// Creates a new Flaps client with the given configuration.
    ///
    /// This will connect to the server and fetch the initial flag configuration.
    pub async fn new(config: Config) -> Result<Self, FlapsError> {
        let client = Self {
            config,
            evaluator: Evaluator::new(),
            flags: Arc::new(RwLock::new(HashMap::new())),
            segments: Arc::new(RwLock::new(HashMap::new())),
        };

        // TODO: Fetch initial flags from server
        // TODO: Start SSE connection or polling

        Ok(client)
    }

    /// Creates a client in offline mode with preloaded flags.
    pub fn offline(flags: Vec<Flag>, segments: Vec<Segment>) -> Self {
        let flags_map: HashMap<String, Flag> =
            flags.into_iter().map(|f| (f.key.0.clone(), f)).collect();
        let segments_map: HashMap<SegmentId, Segment> =
            segments.into_iter().map(|s| (s.id, s)).collect();

        Self {
            config: Config::default().offline(),
            evaluator: Evaluator::with_segments(segments_map.values().cloned().collect()),
            flags: Arc::new(RwLock::new(flags_map)),
            segments: Arc::new(RwLock::new(
                segments_map
                    .into_values()
                    .map(|s| (s.key.clone(), s))
                    .collect(),
            )),
        }
    }

    /// Creates a new evaluation context builder.
    pub fn context(&self) -> EvaluationContext {
        EvaluationContext::new()
    }

    /// Evaluates a flag and returns the full result.
    pub async fn evaluate(&self, flag_key: &str, context: &EvaluationContext) -> EvaluationResult {
        let flags = self.flags.read().await;

        match flags.get(flag_key) {
            Some(flag) => self
                .evaluator
                .evaluate(flag, &self.config.environment, context),
            None => EvaluationResult::flag_not_found(),
        }
    }

    /// Returns true if the flag is enabled for the given context.
    pub async fn is_enabled(&self, flag_key: &str, context: &EvaluationContext) -> bool {
        self.evaluate(flag_key, context).await.is_enabled()
    }

    /// Returns the boolean value of a flag, or the default if not found or disabled.
    pub async fn get_bool(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
        default: bool,
    ) -> bool {
        let result = self.evaluate(flag_key, context).await;
        match result.reason {
            EvaluationReason::FlagNotFound | EvaluationReason::EnvironmentNotFound => default,
            _ => result.value.as_bool().unwrap_or(default),
        }
    }

    /// Returns the string value of a flag, or the default if not found or disabled.
    pub async fn get_string(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
        default: &str,
    ) -> String {
        let result = self.evaluate(flag_key, context).await;
        match result.reason {
            EvaluationReason::FlagNotFound | EvaluationReason::EnvironmentNotFound => {
                default.to_string()
            },
            _ => result
                .value
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| default.to_string()),
        }
    }

    /// Returns all flag keys.
    pub async fn all_flag_keys(&self) -> Vec<String> {
        let flags = self.flags.read().await;
        flags.keys().cloned().collect()
    }

    /// Returns all flags and their current values for debugging.
    pub async fn all_flags(&self, context: &EvaluationContext) -> HashMap<String, FlagValue> {
        let flags = self.flags.read().await;
        let mut results = HashMap::new();

        for (key, flag) in flags.iter() {
            let result = self
                .evaluator
                .evaluate(flag, &self.config.environment, context);
            results.insert(key.clone(), result.value);
        }

        results
    }

    /// Forces a refresh of the flag configuration from the server.
    pub async fn refresh(&self) -> Result<(), FlapsError> {
        if self.config.offline_mode {
            return Ok(());
        }

        // TODO: Fetch flags from server
        // TODO: Update local cache

        Ok(())
    }

    /// Shuts down the client and cleans up resources.
    pub async fn close(&self) {
        // TODO: Close SSE connection
        // TODO: Stop polling
    }
}

/// Errors that can occur when using the Flaps client.
#[derive(Debug, thiserror::Error)]
pub enum FlapsError {
    /// Failed to connect to the server.
    #[error("Connection error: {0}")]
    Connection(String),

    /// Failed to fetch flags.
    #[error("Fetch error: {0}")]
    Fetch(String),

    /// Invalid configuration.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Server returned an error.
    #[error("Server error: {0}")]
    Server(String),
}

#[cfg(test)]
mod tests {
    use flaps_core::{environment::EnvironmentConfig, flag::UserId, project::ProjectId};

    use super::*;

    #[tokio::test]
    async fn test_offline_client() {
        let project_id = ProjectId::new();
        let flags =
            vec![
                Flag::new_boolean("test-flag", "Test Flag", project_id, UserId::new("test"))
                    .with_environment("dev", EnvironmentConfig::enabled_boolean(true)),
            ];

        let client = FlapsClient::offline(flags, vec![]);
        let context = EvaluationContext::with_user_id("user-1");

        assert!(client.is_enabled("test-flag", &context).await);
        assert!(!client.is_enabled("unknown-flag", &context).await);
    }

    #[tokio::test]
    async fn test_get_bool_with_default() {
        let project_id = ProjectId::new();
        let flags =
            vec![
                Flag::new_boolean("enabled-flag", "Enabled", project_id, UserId::new("test"))
                    .with_environment("dev", EnvironmentConfig::enabled_boolean(true)),
            ];

        let client = FlapsClient::offline(flags, vec![]);
        let context = EvaluationContext::new();

        assert!(client.get_bool("enabled-flag", &context, false).await);
        assert!(client.get_bool("unknown-flag", &context, true).await);
        assert!(!client.get_bool("unknown-flag", &context, false).await);
    }
}
