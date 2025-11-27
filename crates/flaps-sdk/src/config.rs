//! SDK configuration.

use serde::{Deserialize, Serialize};

/// Configuration for the Flaps SDK client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// API key for authentication.
    pub api_key: String,
    /// Base URL of the Flaps server.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Environment to evaluate flags in (e.g., "dev", "staging", "prod").
    #[serde(default = "default_environment")]
    pub environment: String,
    /// Project key.
    pub project: Option<String>,
    /// Polling interval in seconds for flag updates.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// Whether to use SSE for real-time updates.
    #[serde(default = "default_use_sse")]
    pub use_sse: bool,
    /// Connection timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Whether to enable offline mode (use cached flags only).
    #[serde(default)]
    pub offline_mode: bool,
}

fn default_base_url() -> String {
    "https://api.flaps.nubster.com".to_string()
}

fn default_environment() -> String {
    "dev".to_string()
}

fn default_poll_interval() -> u64 {
    30
}

fn default_use_sse() -> bool {
    true
}

fn default_timeout() -> u64 {
    10
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: default_base_url(),
            environment: default_environment(),
            project: None,
            poll_interval_secs: default_poll_interval(),
            use_sse: default_use_sse(),
            timeout_secs: default_timeout(),
            offline_mode: false,
        }
    }
}

impl Config {
    /// Creates a new configuration with the required API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Default::default()
        }
    }

    /// Sets the base URL.
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Sets the environment.
    pub fn environment(mut self, env: impl Into<String>) -> Self {
        self.environment = env.into();
        self
    }

    /// Sets the project.
    pub fn project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into());
        self
    }

    /// Enables offline mode.
    pub fn offline(mut self) -> Self {
        self.offline_mode = true;
        self
    }
}
