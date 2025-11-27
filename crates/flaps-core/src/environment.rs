//! Environment types and configuration.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::flag::FlagValue;
use crate::project::ProjectId;
use crate::rule::TargetingRule;

/// Unique identifier for an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvironmentId(pub Uuid);

impl EnvironmentId {
    /// Creates a new random environment ID.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Creates an environment ID from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for EnvironmentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EnvironmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An environment where flags can be evaluated (e.g., dev, staging, prod).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    /// Unique identifier.
    pub id: EnvironmentId,
    /// Machine-readable key (e.g., "dev", "staging", "prod").
    pub key: String,
    /// Display name (e.g., "Development", "Staging", "Production").
    pub name: String,
    /// Optional color for UI display (e.g., "#22c55e" for green).
    pub color: Option<String>,
    /// Whether this is a production environment (may require approval for changes).
    pub is_production: bool,
    /// Project this environment belongs to.
    pub project_id: ProjectId,
    /// Order for display purposes.
    pub order: u32,
}

impl Environment {
    /// Creates a new environment.
    pub fn new(
        key: impl Into<String>,
        name: impl Into<String>,
        project_id: ProjectId,
    ) -> Self {
        Self {
            id: EnvironmentId::new(),
            key: key.into(),
            name: name.into(),
            color: None,
            is_production: false,
            project_id,
            order: 0,
        }
    }

    /// Creates a development environment.
    pub fn development(project_id: ProjectId) -> Self {
        Self {
            id: EnvironmentId::new(),
            key: "dev".to_string(),
            name: "Development".to_string(),
            color: Some("#22c55e".to_string()), // Green
            is_production: false,
            project_id,
            order: 0,
        }
    }

    /// Creates a staging environment.
    pub fn staging(project_id: ProjectId) -> Self {
        Self {
            id: EnvironmentId::new(),
            key: "staging".to_string(),
            name: "Staging".to_string(),
            color: Some("#f59e0b".to_string()), // Orange
            is_production: false,
            project_id,
            order: 1,
        }
    }

    /// Creates a production environment.
    pub fn production(project_id: ProjectId) -> Self {
        Self {
            id: EnvironmentId::new(),
            key: "prod".to_string(),
            name: "Production".to_string(),
            color: Some("#ef4444".to_string()), // Red
            is_production: true,
            project_id,
            order: 2,
        }
    }

    /// Sets the color.
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Marks as production environment.
    pub fn with_production(mut self, is_production: bool) -> Self {
        self.is_production = is_production;
        self
    }

    /// Sets the display order.
    pub fn with_order(mut self, order: u32) -> Self {
        self.order = order;
        self
    }
}

/// Configuration of a flag for a specific environment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvironmentConfig {
    /// Whether the flag is enabled in this environment.
    pub enabled: bool,
    /// Targeting rules evaluated in order of priority.
    pub rules: Vec<TargetingRule>,
    /// Default value when no rules match and flag is enabled.
    pub default_value: FlagValue,
    /// Global rollout percentage (0-100). Applied after rules evaluation.
    pub rollout_percentage: Option<u8>,
    /// Whether changes require approval.
    pub requires_approval: bool,
}

impl EnvironmentConfig {
    /// Creates a new disabled environment config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a disabled config explicitly.
    ///
    /// This is semantically clearer than `new()` when you want to express
    /// that a flag is intentionally disabled in an environment.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            rules: Vec::new(),
            default_value: FlagValue::Boolean(false),
            rollout_percentage: None,
            requires_approval: false,
        }
    }

    /// Creates an enabled config with a boolean default value.
    pub fn enabled_boolean(value: bool) -> Self {
        Self {
            enabled: true,
            rules: Vec::new(),
            default_value: FlagValue::Boolean(value),
            rollout_percentage: None,
            requires_approval: false,
        }
    }

    /// Creates an enabled config with a string default value.
    pub fn enabled_string(value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            rules: Vec::new(),
            default_value: FlagValue::String(value.into()),
            rollout_percentage: None,
            requires_approval: false,
        }
    }

    /// Sets the enabled state.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets the default value.
    pub fn with_default_value(mut self, value: FlagValue) -> Self {
        self.default_value = value;
        self
    }

    /// Sets the rollout percentage.
    pub fn with_rollout(mut self, percentage: u8) -> Self {
        self.rollout_percentage = Some(percentage.min(100));
        self
    }

    /// Adds a targeting rule.
    pub fn with_rule(mut self, rule: TargetingRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Sets whether approval is required.
    pub fn with_approval_required(mut self, required: bool) -> Self {
        self.requires_approval = required;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_environments() {
        let project_id = ProjectId::new();

        let dev = Environment::development(project_id);
        assert_eq!(dev.key, "dev");
        assert!(!dev.is_production);

        let staging = Environment::staging(project_id);
        assert_eq!(staging.key, "staging");
        assert!(!staging.is_production);

        let prod = Environment::production(project_id);
        assert_eq!(prod.key, "prod");
        assert!(prod.is_production);
    }

    #[test]
    fn test_environment_config() {
        let config = EnvironmentConfig::enabled_boolean(true)
            .with_rollout(50)
            .with_approval_required(true);

        assert!(config.enabled);
        assert_eq!(config.rollout_percentage, Some(50));
        assert!(config.requires_approval);
    }
}
