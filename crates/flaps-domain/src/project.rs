//! Project aggregate: top-level organisational unit in Flaps.

use serde::{Deserialize, Serialize};

use crate::{
    federation::{ExternalRef, ManagedBy},
    key::ProjectKey,
};

/// A project groups environments, flags and segments under a shared namespace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Unique identifier.
    pub key: ProjectKey,
    /// Human-readable display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Opaque reference to the project in a federated system.
    pub external_ref: Option<ExternalRef>,
    /// Whether this project is owned locally or replicated from a federation.
    pub managed_by: ManagedBy,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        federation::{ExternalRef, ManagedBy},
        key::ProjectKey,
    };

    #[test]
    fn external_ref_optional() {
        let project = Project {
            key: ProjectKey::new("my-project").unwrap(),
            name: "My Project".into(),
            description: None,
            external_ref: None,
            managed_by: ManagedBy::Local,
        };
        assert!(project.external_ref.is_none());
    }

    #[test]
    fn managed_by_federated() {
        let project = Project {
            key: ProjectKey::new("fed-project").unwrap(),
            name: "Fed".into(),
            description: None,
            external_ref: Some(ExternalRef::new("urn:ext:abc123")),
            managed_by: ManagedBy::Federated,
        };
        assert_eq!(project.managed_by, ManagedBy::Federated);
        assert_eq!(
            project.external_ref.as_ref().unwrap().as_str(),
            "urn:ext:abc123"
        );
    }

    #[test]
    fn serde_round_trip() {
        let project = Project {
            key: ProjectKey::new("my-project").unwrap(),
            name: "My Project".into(),
            description: Some("desc".into()),
            external_ref: Some(ExternalRef::new("ref-1")),
            managed_by: ManagedBy::Local,
        };
        let json = serde_json::to_string(&project).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, project);
    }
}
