//! Environment aggregate: a named deployment target within a project.

use serde::{Deserialize, Serialize};

use crate::{
    federation::{ExternalRef, ManagedBy},
    key::EnvironmentKey,
    metadata::Metadata,
};

/// A named deployment environment (e.g. `production`, `staging`, `dev`).
///
/// Each environment has its own [`FlagEnvConfig`](crate::flag_env_config::FlagEnvConfig)
/// per flag, allowing independent targeting and rollout rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    /// Unique identifier within the project.
    pub key: EnvironmentKey,
    /// Human-readable display name.
    pub name: String,
    /// Opaque reference to the environment in a federated system.
    pub external_ref: Option<ExternalRef>,
    /// Whether this environment is owned locally or replicated from a federation.
    pub managed_by: ManagedBy,
    /// Arbitrary flag-set-level metadata, merged into the resolution metadata
    /// at evaluation time (flag entries win over these on collision).
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        federation::{ExternalRef, ManagedBy},
        key::EnvironmentKey,
    };

    #[test]
    fn external_ref_optional() {
        let env = Environment {
            key: EnvironmentKey::new("production").unwrap(),
            name: "Production".into(),
            external_ref: None,
            managed_by: ManagedBy::Local,
            metadata: Metadata::new(),
        };
        assert!(env.external_ref.is_none());
    }

    #[test]
    fn managed_by_federated() {
        let env = Environment {
            key: EnvironmentKey::new("staging").unwrap(),
            name: "Staging".into(),
            external_ref: Some(ExternalRef::new("urn:env:staging")),
            managed_by: ManagedBy::Federated,
            metadata: Metadata::new(),
        };
        assert_eq!(env.managed_by, ManagedBy::Federated);
    }

    #[test]
    fn serde_round_trip() {
        let env = Environment {
            key: EnvironmentKey::new("dev").unwrap(),
            name: "Development".into(),
            external_ref: None,
            managed_by: ManagedBy::Local,
            metadata: Metadata::new(),
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: Environment = serde_json::from_str(&json).unwrap();
        assert_eq!(back, env);
    }

    #[test]
    fn serde_round_trip_preserves_metadata() {
        let mut env = Environment {
            key: EnvironmentKey::new("prod").unwrap(),
            name: "Production".into(),
            external_ref: None,
            managed_by: ManagedBy::Local,
            metadata: Metadata::new(),
        };
        env.metadata.insert(
            "region".to_owned(),
            crate::metadata::MetadataValue::String("eu-west".into()),
        );
        env.metadata.insert(
            "critical".to_owned(),
            crate::metadata::MetadataValue::Bool(true),
        );

        let json = serde_json::to_string(&env).unwrap();
        let back: Environment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.metadata, env.metadata);
    }

    #[test]
    fn metadata_absent_from_json_deserializes_to_empty_map() {
        // Proves #[serde(default)] backward compatibility: an Environment
        // persisted before metadata existed must still deserialize successfully.
        let json = serde_json::json!({
            "key": "legacy-env",
            "name": "Legacy",
            "external_ref": null,
            "managed_by": "local"
        })
        .to_string();
        let env: Environment = serde_json::from_str(&json).unwrap();
        assert!(env.metadata.is_empty());
    }

    #[test]
    fn empty_metadata_is_omitted_from_serialized_json() {
        let env = Environment {
            key: EnvironmentKey::new("prod").unwrap(),
            name: "Production".into(),
            external_ref: None,
            managed_by: ManagedBy::Local,
            metadata: Metadata::new(),
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(
            !json.contains("\"metadata\""),
            "empty metadata must not be serialized: {json}"
        );
    }
}
