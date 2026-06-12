//! Environment aggregate: a named deployment target within a project.

use serde::{Deserialize, Serialize};

use crate::{
    federation::{ExternalRef, ManagedBy},
    key::EnvironmentKey,
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
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: Environment = serde_json::from_str(&json).unwrap();
        assert_eq!(back, env);
    }
}
