//! Feature flag types and structures.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::environment::EnvironmentConfig;
use crate::project::ProjectId;

/// Unique identifier for a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FlagId(pub Uuid);

impl FlagId {
    /// Creates a new random flag ID.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Creates a flag ID from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for FlagId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for FlagId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Human-readable key for a flag (e.g., "new-checkout", "dark-mode").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FlagKey(pub String);

impl FlagKey {
    /// Creates a new flag key.
    ///
    /// # Panics
    ///
    /// Panics if the key is empty or contains invalid characters.
    pub fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        assert!(!key.is_empty(), "Flag key cannot be empty");
        assert!(
            key.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "Flag key can only contain alphanumeric characters, hyphens, and underscores"
        );
        Self(key)
    }

    /// Tries to create a new flag key, returning None if invalid.
    pub fn try_new(key: impl Into<String>) -> Option<Self> {
        let key = key.into();
        if key.is_empty() {
            return None;
        }
        if !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }
        Some(Self(key))
    }

    /// Returns the key as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for FlagKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for FlagKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// User ID for audit purposes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub String);

impl UserId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// Type of value a flag can return.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "variants")]
pub enum FlagType {
    /// Simple on/off flag.
    Boolean,
    /// String flag with defined variants (for A/B testing).
    String { variants: Vec<String> },
}

impl Default for FlagType {
    fn default() -> Self {
        Self::Boolean
    }
}

/// Value returned by a flag evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FlagValue {
    Boolean(bool),
    String(String),
}

impl FlagValue {
    /// Returns the boolean value if this is a boolean flag.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            FlagValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns the string value if this is a string flag.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            FlagValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Returns true if this is a truthy value.
    pub fn is_truthy(&self) -> bool {
        match self {
            FlagValue::Boolean(b) => *b,
            FlagValue::String(s) => !s.is_empty(),
        }
    }
}

impl Default for FlagValue {
    fn default() -> Self {
        Self::Boolean(false)
    }
}

impl From<bool> for FlagValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<String> for FlagValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for FlagValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

/// A feature flag with targeting rules and environment configurations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flag {
    /// Unique identifier.
    pub id: FlagId,
    /// Human-readable key (e.g., "new-checkout").
    pub key: FlagKey,
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Type of flag (boolean or string with variants).
    pub flag_type: FlagType,
    /// Configuration per environment.
    pub environments: HashMap<String, EnvironmentConfig>,
    /// Tags for organization.
    pub tags: Vec<String>,
    /// Project this flag belongs to.
    pub project_id: ProjectId,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
    /// User who created this flag.
    pub created_by: UserId,
}

impl Flag {
    /// Creates a new boolean flag.
    pub fn new_boolean(
        key: impl Into<String>,
        name: impl Into<String>,
        project_id: ProjectId,
        created_by: UserId,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: FlagId::new(),
            key: FlagKey::new(key),
            name: name.into(),
            description: None,
            flag_type: FlagType::Boolean,
            environments: HashMap::new(),
            tags: Vec::new(),
            project_id,
            created_at: now,
            updated_at: now,
            created_by,
        }
    }

    /// Creates a new string flag with variants.
    pub fn new_string(
        key: impl Into<String>,
        name: impl Into<String>,
        variants: Vec<String>,
        project_id: ProjectId,
        created_by: UserId,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: FlagId::new(),
            key: FlagKey::new(key),
            name: name.into(),
            description: None,
            flag_type: FlagType::String { variants },
            environments: HashMap::new(),
            tags: Vec::new(),
            project_id,
            created_at: now,
            updated_at: now,
            created_by,
        }
    }

    /// Sets the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Adds a tag.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Adds an environment configuration.
    pub fn with_environment(
        mut self,
        env_key: impl Into<String>,
        config: EnvironmentConfig,
    ) -> Self {
        self.environments.insert(env_key.into(), config);
        self
    }

    /// Gets the configuration for a specific environment.
    pub fn get_environment(&self, env_key: &str) -> Option<&EnvironmentConfig> {
        self.environments.get(env_key)
    }

    /// Returns the default value based on flag type.
    pub fn default_value(&self) -> FlagValue {
        match &self.flag_type {
            FlagType::Boolean => FlagValue::Boolean(false),
            FlagType::String { variants } => {
                FlagValue::String(variants.first().cloned().unwrap_or_default())
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flag_key_validation() {
        assert!(FlagKey::try_new("valid-key").is_some());
        assert!(FlagKey::try_new("valid_key").is_some());
        assert!(FlagKey::try_new("validKey123").is_some());
        assert!(FlagKey::try_new("").is_none());
        assert!(FlagKey::try_new("invalid key").is_none());
        assert!(FlagKey::try_new("invalid.key").is_none());
    }

    #[test]
    fn test_flag_value_conversions() {
        let bool_val: FlagValue = true.into();
        assert_eq!(bool_val.as_bool(), Some(true));
        assert!(bool_val.is_truthy());

        let str_val: FlagValue = "variant-a".into();
        assert_eq!(str_val.as_str(), Some("variant-a"));
        assert!(str_val.is_truthy());

        let empty_str: FlagValue = "".into();
        assert!(!empty_str.is_truthy());
    }

    #[test]
    fn test_create_boolean_flag() {
        let flag = Flag::new_boolean(
            "test-flag",
            "Test Flag",
            ProjectId::new(),
            UserId::new("user-1"),
        );

        assert_eq!(flag.key.as_str(), "test-flag");
        assert_eq!(flag.name, "Test Flag");
        assert_eq!(flag.flag_type, FlagType::Boolean);
    }
}
